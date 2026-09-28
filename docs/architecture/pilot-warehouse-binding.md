# Pilot contract — warehouse binding (CP-REFACTOR-030)

**Status.** Prepared in CP-REFACTOR-010; **executed in CP-REFACTOR-030** (2026-09-26) on base
tree `9889036ee786dc90d2e44d7aeeb80fa62efe1a5d`. The binding implementation now lives in the
child module described below; the sections that follow are the contract the slice had to keep,
followed by what the executed slice actually did.

## Result of the executed slice

| Before | After |
| --- | --- |
| binding types, validation, lookup and the three `bind_*` methods inside `script/storage/settlement.rs` (4040 lines) | `script/storage/settlement/warehouse.rs` (526 lines) holding `DurableWarehouseSource`/`DurableWarehouseBinding` (`:19/:38`), the ledger lookups (`:82 binding_position`, `:101 warehouse_for_position`), validation (`:113 validate_warehouse_binding`), cross-kind identity (`:147 same_warehouse_target`), handle minting (`:197 manual_warehouse_handle`), authored position resolution (`:216 warehouse_container_position`) and the three binds (`:262/:346/:440`) |
| three manual-bind tests inside `settlement_tests.rs` (6088 lines) | `script/storage/settlement/warehouse_tests.rs` (260 lines), declared as a child of `settlement_tests` with `#[path = "settlement/warehouse_tests.rs"] mod warehouse_tests;` — so they reuse the parent's private `Fixture`/helpers with `use super::*`, no second fixture and no widened test visibility |
| — | `settlement.rs` stays the module root (3544 lines) and keeps the ledger authority, the shared atomic `apply`, survey/construction/certificate logic; it declares `mod warehouse;` and imports only `DurableWarehouseBinding`, `DurableWarehouseSource`, `same_warehouse_target`, `validate_warehouse_binding`, `warehouse_container_position` |

Visibility: the two moved types are `pub(in crate::script::storage)` (the parent exposes
`DurableSettlementChange::Warehouse` and `SettlementLedger::warehouse` at that level), their
fields are `pub(super)`, and the bind methods are `pub(super)` on `InventoryRuntime`. Nothing
was widened to `pub(crate)`, and no forwarding wrapper, second ledger or re-export was added.

Behaviour is unchanged by construction (pure move) and by evidence: the three native tests and
both refusal tests pass with the same outcomes, and the Rust-facing call sites are the same
dispatch arms in `settlement.rs`. The scenario catalog now runs them as
`warehouse-bind-manual` / `warehouse-bind-refusals` with the new module path.

**Not proven by this slice.** The `settlement-component-cargo` integration route is currently
blocked by a pre-existing component-hash mismatch in the sibling repository (see the
CP-REFACTOR-020 receipt); it was re-run as part of this checkpoint and fails closed before any
component test runs, so the component consumer is not evidenced here. Manual/graphical
acceptance of manual building remains CP-049 debt.

## Outcome (original contract)

A change to manual/authored/village warehouse binding can be made and reviewed from one local
contract plus a narrow set of implementation and test files, without reading all of
`crates/mc-net/src/script/storage/settlement.rs` (4040 lines) and
`settlement_tests.rs` (6088 lines).

## Baseline cost (measured in CP-REFACTOR-000, receipt `.analysis/codex-logs/cprefactor-000/`)

- finding the writer and every refusal took 3 navigation hops;
- the reviewer/human must currently read ~10.1k lines (implementation + tests) to review one bind change;
- focused native cycle: `python3 -m tools.harness run test -- -p mc-net --lib manual` = 58.3 s
  rebuild + 0.38 s for 3 tests (build-bound; splitting test files does not by itself shorten it).

## Current code and callers (verified, after the CP-REFACTOR-030 move)

| Concern | Symbol |
| --- | --- |
| durable binding record | `DurableWarehouseBinding` (`script/storage/settlement/warehouse.rs:38`), `DurableWarehouseSource` (`:19`) |
| ledger container | `SettlementLedger.warehouses` (`script/storage/settlement.rs`) |
| position resolution (authored → structure/catalog) | `binding_position` (`warehouse.rs:82`) |
| one-container-one-binding lookup | `warehouse_for_position` (`warehouse.rs:101`) |
| durable validation | `validate_warehouse_binding` (`warehouse.rs:113`), `validate_warehouse_position` (`:133`) |
| journal apply + rebind/revision rules | `SettlementLedger::apply`, `Warehouse` arm (parent `settlement.rs`) |
| same-target comparison | `same_warehouse_target` (`warehouse.rs:147`) |
| dispatch by operation | parent `settlement.rs` → `bind_warehouse` (`warehouse.rs:262`), `bind_village_warehouse` (`:346`), `bind_manual_warehouse` (`:440`) |
| deterministic manual handle | `manual_warehouse_handle` (`warehouse.rs:197`) |
| DTO / result | `ScriptWarehouseBinding` (`crates/mc-script/src/settlement_operations.rs:1135`, validation `:1196`); the operation input for the three binds is validated in the same file (`:1441-1450`) |
| host conversion | `crates/mc-plugin-host/src/domain_settlements.rs` |
| SDK surface | `sdk/rust/solaris-plugin-sdk/src/settlement_ops.rs:321/344/367` (+ example `sdk/rust/examples/hello/src/settlement_ops.rs`) |
| native tests | `script/storage/settlement/warehouse_tests.rs:8/79/215` via `settlement_tests::warehouse_tests`; catalog ids `warehouse-bind-manual` / `warehouse-bind-refusals` |

## Preserved semantics (must not change in this slice)

1. `binding_position` / `warehouse_for_position` stay fallible `Option`; an authored binding
   whose structure/catalog no longer resolves is skipped by the cross-kind check instead of
   failing every later bind (no new global failure).
2. Refusal precedence inside `bind_manual_warehouse` stays exactly as ordered today:
   world → token existence → foreign plugin → non-overworld → bounds → claims → container
   availability (**before** freshness) → expiry/footprint → existing binding → handle → commit.
3. One resolved container carries at most one binding across kinds (`same_warehouse_target`,
   plus the `apply` rebind rejection).
4. `manual_warehouse_handle` stays deterministic, so re-binding the same position returns the
   same handle (idempotence) and the existing binding is returned to its owner.
5. Journal/serde contract unchanged: `DurableSettlementChange::Warehouse` payload, field names,
   `validate_warehouse_binding` bounds, revision/rebind rules.
6. Refusal before the durable decision changes nothing; the response is not the commit.

## Allowed write set

Minimal first cut — extract only the binding code and the tests that exercise it:

```text
crates/mc-net/src/script/storage/settlement.rs   stays the module root: declares
                                                 `mod warehouse;` and keeps the ledger authority,
                                                 the shared atomic `apply`, and every
                                                 non-warehouse concern
crates/mc-net/src/script/storage/settlement/
  warehouse.rs       source resolution + bind validation + binding operations
  warehouse_tests.rs the three manual-bind tests (plus their private helpers)
crates/mc-net/src/script/storage/settlement_tests.rs  the moved tests leave this file
crates/mc-net/src/script/storage.rs              only if a caller/import actually changes
                                                 (the child module is declared in settlement.rs)
tools/harness/scenarios.py                       the `warehouse-bind-manual` / `warehouse-bind-refusals`
                                                 catalog entries (module owned by CP-REFACTOR-020)
docs/architecture/pilot-warehouse-binding.md     results of the slice
docs/architecture/mutations.md                   card 1 line references after the move
```

Rust rejects a file and a same-named `mod.rs` in one module (E0761), so `settlement.rs` must
stay the root; the added directory only holds child modules — the same shape the tree already
uses for `world_inventory.rs` + `world_inventory/`.

Do **not** pre-emptively move `SettlementLedger`, the atomic `apply`, or the shared settlement
fixture out of `settlement.rs`, and do not create `ledger.rs`/`test_support.rs` speculatively:
add a file only when a concrete compile, privacy or fixture-sharing blocker requires it, and
record why in this card. Do **not** create a second ledger or a duplicate fixture.

## Forbidden in this slice

- WIT shapes, wire ids/layouts, journal/NBT/serde field names, storage keys;
- owner lock order, durable append/sync order, receipt/replay ordering;
- survey, construction and certificate logic (they may stay in the same file for now);
- new crates, compatibility re-exports, forwarding modules, generic transaction frameworks;
- behaviour changes dressed as a move; a found bug becomes its own slice with a reproduction.

## Steps

1. Reuse the CP-REFACTOR-000 baseline receipts (cold 58.3 s build + 0.38 s for the three
   tests; warm unchanged 0.6 s) instead of re-running the unchanged gate; after the move run
   the focused tests once, and rerun only after a relevant fix. Compare against that baseline.
2. Move the binding code with minimal visibility (`pub(super)`/`pub(crate)` as needed).
3. Update every caller, import, test selector and the scenario catalog in the same slice.
4. Delete replaced helpers/re-exports; no forwarding compatibility path.
5. Re-run the focused tests, then the existing component path (`settlement-cargo`).
6. Compare the developer-task cost against the CP-REFACTOR-000 baseline; if hops, wrappers,
   duplicated registries or fixture dependencies grew, fix the shape before spreading it.

## Verification

- native: `cargo test -p mc-net --lib settlement_tests::warehouse_tests` (or the catalog ids
  `warehouse-bind-manual` / `warehouse-bind-refusals`), unchanged outcomes;
- component: `python3 -m tools.harness run settlement-cargo` (rebuilds and hash-verifies the
  sibling component, then the host/native suites) — complements, does not prove, CP-049;
- named run: `warehouse-bind-manual` / `warehouse-bind-refusals` once CP-REFACTOR-020 lands;
- docs: this file and `mutations.md` card 1 updated in the same slice;
- review: one independent read-only reviewer over the bounded diff.
