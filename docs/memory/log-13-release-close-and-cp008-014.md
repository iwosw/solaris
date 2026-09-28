# Archived checkpoint log — part 13 of 17

Era: 09-18/09-19. Chronological checkpoint history moved out of `docs/MEMORY.md` so
the live cursor stays small. **Not startup context** (see `AGENTS.md`); read it when
a question is about this era, not to learn the current state.

## Sections in this part

- v0.0.7 release close (2026-09-18)
- Checkpoint — CP-008 real agricultural cycle (2026-09-18)
- Checkpoint — CP-009 real-tree-constrained logging (2026-09-18)
- Checkpoint — CP-010 honest resident mining (2026-09-18)
- Checkpoint — CP-011 fishing and livestock resource contract (2026-09-18)
- Checkpoint — CP-012 station-backed warehouse production (2026-09-18)
- Checkpoint — CP-013 event-gated resident work resumption closed (2026-09-19)
- Checkpoint — v0.0.7 release CI repair, iterations 1-3 (2026-09-19)
- Active checkpoint — CP-014 canonical R1/R0 chain scenario (partial)

---

## v0.0.7 release close (2026-09-18)

**State.** The requested v0.0.7 release was published as `9e5e20e4` and tag
`v0.0.7`. It
adds passive-animal food temptation, sight-gated skeleton bow attacks, vanilla
chest lid block events, and documented first-start Mojang content-cache reuse.
Settlement policy remains in plugins; core only provides the generic durable
world/inventory transaction boundary.

**Evidence.** Canonical `correctness` passed at
`.analysis/validation/20260918T183211-correctness-0m_g7trd`; focused
regressions cover fractional skeleton sight traversal, blocked draw cancellation,
cross-region chest commits, durable settlement portions, and TNT damage to an
immobile chicken. Release preflight passed: installer
`20260918T182514-installer-xx4oj04t`, harness wiring
`20260918T182514-harness-check-xu1vbvf6`, and release build
`20260918T182514-build-bnct9702`.

**Boundary.** Graphical real-client, oracle, and performance gates remain
unrun, so v0.0.7 is draft. Next active task: CP-010.

## Checkpoint — CP-008 real agricultural cycle (2026-09-18)

**Outcome.** Resident `Harvest` previews canonical drops before changing a crop;
`Replant` consumes a configured seed only in the same durable world decision
that places the crop and advances work. Both paths use source-image
preconditions, fail closed for missing tool/input/unloaded terrain, and split
at the simulation's 512-edit boundary. This is generic worker/world
transaction machinery, not settlement policy.

```yaml
base_tree: 9e5e20e4b53dbb769a38f6b1e54e4257609e9ec9
diff_hash: 1b0dbccef47da92a59861b7ec2105369886621f81d1cb31a6e402e78139d1652
changed_files:
  - crates/mc-net/src/play/resident_work.rs
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
validation:
  - CP-008 harvest/replant, missing seed/tool, stale player edit, and 512-edit split:
      passed
  - .analysis/validation/20260918T190426-fmt-0pftdzmu/result.json: passed
  - .analysis/validation/20260918T190426-code-health-jebmaofu/result.json: passed
  - CP008AgricultureReview: pass; no findings
next: Begin CP-009 real-tree-constrained logging.
```

## Checkpoint — CP-009 real-tree-constrained logging (2026-09-18)

**Outcome.** `CutTree` accepts only a bounded rooted trunk crowned by a real
canopy block, so an adjacent grounded log column without connected canopy
evidence remains untouched. The tree still uses the existing source-image
preview, durable world decision, cargo, tool, capacity, route, and permission
boundaries; unloaded classifier reads fail closed.

```yaml
base_tree: 9e5e20e4b53dbb769a38f6b1e54e4257609e9ec9
diff_hash: 3bc2838d5b4ace37ec6165c65415dd3bf6297173b40d4ee07fa34c1f2c727295
changed_files:
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
validation:
  - cut_tree_commits_the_log_and_worker_cargo_together: passed
  - cut_tree_keeps_adjacent_grounded_house_logs: passed
  - .analysis/validation/20260918T191534-fmt-c1a1_isw/result.json: passed
  - .analysis/validation/20260918T191534-code-health-2gi9aje8/result.json: passed
  - CP009LoggingReview: finding fixed; no second review by policy
next: Begin CP-010 honest mining.
```

## Checkpoint — CP-010 honest resident mining (2026-09-18)

**Outcome.** `Mine` visits only the named, loaded work cells and obtains a
source-conditioned canonical break preview. A tool that produces no canonical
drops leaves its ore intact and pauses `MissingTool`, including when it follows
an already committed valid break. A full worker likewise leaves the ore intact
with `NoStorage`; no profession grants ore or scans for hidden resources.

```yaml
base_tree: 9e5e20e4b53dbb769a38f6b1e54e4257609e9ec9
diff_hash: ca9573fce49b628752d128c424a61127e2ea42607358d4d84e94d4dafbb36d9e
changed_files:
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
validation:
  - mined_ore_reaches_the_worker_cargo: passed
  - mine_with_unsuitable_tool_preserves_ore: passed
  - mine_pauses_for_a_later_ore_that_needs_a_better_tool: passed
  - mine_with_full_cargo_leaves_ore_untouched: passed
  - .analysis/validation/20260918T192546-fmt-mfrwyyox/result.json: passed
  - .analysis/validation/20260918T192546-code-health-7ea4rzeb/result.json: passed
  - CP010MiningReview: MissingTool after partial progress fixed; no second review by policy
next: Begin CP-011 fishing and animal-resource contract.
```

## Checkpoint — CP-011 fishing and livestock resource contract (2026-09-18)

**Outcome.** `Fish` credits the resident only from named loaded water columns;
its deterministic cod output is explicitly a bounded worker abstraction, not
vanilla fishing loot/timing parity. `TendLivestock` now admits only an alive
locally tracked adult accepted by the canonical food tags, then debits the
resident's own feed and mints no livestock goods. Missing water, animals,
compatible food, or maturity leaves inputs untouched with a typed pause.

```yaml
base_tree: 9e5e20e4b53dbb769a38f6b1e54e4257609e9ec9
diff_hash: 6a5710ccf6cfc51ca94dc02183b2024d8c89b42b9160cb2915ff86bfcd86c508
changed_files:
  - crates/mc-net/src/play.rs
  - crates/mc-net/src/play/session/resident_orders.rs
  - crates/mc-net/src/play/simulation.rs
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
validation:
  - fish_uses_real_water_and_credits_resident_cargo: passed
  - tend_livestock_consumes_feed_for_a_visible_animal: passed
  - tend_livestock_refuses_incompatible_or_immature_animals: passed
  - fish_and_livestock_without_sources_preserve_inputs: passed
  - .analysis/validation/20260918T194406-fmt-i9p3_95b/result.json: passed
  - .analysis/validation/20260918T194406-code-health-hx1ld9ur/result.json: passed
  - CP011ResourceReview: pass; no findings
next: Begin CP-012 warehouse-backed production.
```

## Checkpoint — CP-012 station-backed warehouse production (2026-09-18)

**Outcome.** The owner selected a mandatory station reference. Every `Craft`
now names one singleton world cell; the native executor verifies a loaded
`crafting_table` before material debit, reporting `missing_station`, `unloaded`,
or `protected` without inventing output. The existing durable chain remains
explicit: warehouse withdrawal → craft → warehouse deposit. Resident output
placement proves its full capacity before any slot changes. A failed craft
therefore restores every tentatively removed ingredient and leaves no partial
output before reporting `no_storage`. The exact craft receipt replays without a
second recipe decision.

```yaml
base_tree: 9e5e20e4b53dbb769a38f6b1e54e4257609e9ec9
diff_hash: d3a132b54d0a5a06287aa7dc6c72d546d679544adbde29c341e3df23b275010c
changed_files:
  - crates/mc-script/wit/residents.wit
  - crates/mc-script/src/resident_order_operations.rs
  - crates/mc-script/src/resident_order_operations_tests.rs
  - crates/mc-plugin-host/src/domain_residents.rs
  - crates/mc-net/src/script/storage/resident_order_execution.rs
  - crates/mc-net/src/script/storage/resident_order_tests.rs
  - crates/mc-net/src/script/storage/resident_settlement_tests.rs
validation:
  - work_orders_require_a_concrete_bounded_target: passed
  - craft_requires_inputs_and_consumes_them_exactly_once: passed
  - craft_requires_the_named_crafting_table: passed
  - workshop_returns_warehouse_inputs_as_one_real_recipe_output: passed
  - mc-plugin-host domain-residents compile: passed
  - craft_without_output_capacity_preserves_ingredients: passed
  - cargo test -p mc-net --lib craft: 51 passed
  - .analysis/validation/20260919T025512-fmt-v_atb9ld/result.json: passed
  - .analysis/validation/20260919T025522-code-health-0qjqmzan/result.json: passed
  - .analysis/validation/20260918T220935-fmt-v2rp0oy1/result.json: passed
  - .analysis/validation/20260918T220935-code-health-0l0r1dk8/result.json: passed
  - CP012WorkshopReview: pass; no findings
next: Implement owner-selected native event-gated CP-013 work resumption.
```

## Checkpoint — CP-013 event-gated resident work resumption closed (2026-09-19)

**Outcome.** Paused durable resident work resumes only from a causal event for
its exact prerequisite: world/chunk edits, chunk publication, owned-inventory
transfers (resident endpoints plus the exact-destination warehouse index), and
accepted protection-zone mutations. Each wake re-runs the original assignment
once from its committed watermark and delivers the native receipt; repeated or
non-overlapping events select nothing; an inactive owner route leaves work
paused; a reopened Store retains the exact paused job. The component Store
reload proof shows the replacement generation starts with fresh host globals
and timer schedules (fresh `probe-init`/`probe-join-1` fires) while durable
state stays core-owned.

```yaml
base_tree: 9e5e20e4b53dbb769a38f6b1e54e4257609e9ec9
diff_hash: 49889f13afc9068d8012b9c581987e0b549bf845092fc7ed728aa3963ddf4695
changed_files:
  - crates/mc-script/src/lib.rs
  - crates/mc-net/src/connection_driver.rs
  - crates/mc-net/src/server.rs
  - crates/mc-net/src/play.rs
  - crates/mc-net/src/play/block_edit_commit.rs
  - crates/mc-net/src/play/chunk_stream.rs
  - crates/mc-net/src/play/tests.rs
  - crates/mc-net/src/play/tests/{tab_list,outbound_delivery,scheduled_hoppers}.rs
  - crates/mc-net/src/script/router.rs
  - crates/mc-net/src/script/zone.rs
  - crates/mc-net/src/script/storage.rs
  - crates/mc-net/src/script/storage/{resident_order_execution,resident_orders,world_inventory}.rs
  - crates/mc-net/src/script/storage/{resident_order_tests,resident_settlement_tests}.rs
  - crates/mc-plugin-host/tests/host_deployment.rs
  - sdk/rust/examples/hello/src/resident_ops.rs
validation:
  - world_event_resumes_the_same_paused_craft_once: passed
  - chunk_load_event_resumes_the_same_unloaded_craft_once: passed
  - inventory_event_resumes_the_same_paused_harvest_once: passed
  - zone_change_resumes_the_same_protected_harvest_once: passed
  - storage_actor_delivers_one_native_resume_receipt: passed
  - warehouse_change_resumes_the_same_paused_haul_once: passed
  - compatible_reload_restarts_the_runtime_timer_store: passed
  - .analysis/validation/20260919T040028-fmt-dy1_4tl1/result.json: passed
  - .analysis/validation/20260919T040037-code-health-b2uap84q/result.json: passed
next: v0.0.8 core+survival outcomes (owner directive); L2 before any release close.
```

## Checkpoint — v0.0.7 release CI repair, iterations 1-3 (2026-09-19)

**Outcome.** Main is red-gate-free by local evidence across four root causes:
(1) the v0.0.7 release commit never added the precommit/WASM host source set —
landed complete in `7f68cc2a`; (2) the Loader job pinned the pre-wire-3 loader
`3aaa9266` — re-pinned to `0972926`; (3) the runner lacked the wasm32 target and
both heavy jobs died with the cgroup exit-241 kill under the 4G scope — test and
Loader jobs now run with an 8G cap; (4) the shipped live-gate fixture carried the
unfinished loader cycle's `input_bindings`, `sounds` and prefixed action ids,
which the pinned wire-3 index refuses — the fixture is realigned to the pinned
contract (sounds/bindings/audio tooling stripped, `confirm` unprefixed) and
proven green against the pinned loader-core suite plus the full six-module
gradle command in clean worktrees. The loader side's own `input_bindings`/sounds
cycle stays as uncommitted sibling work for its own checkpoint.

## Active checkpoint — CP-014 canonical R1/R0 chain scenario (partial)

**Landed (uncommitted).** `crates/mc-test-harness/tests/wasm_settlement_operations.rs`
gains `existing_village_walks_the_chain_and_survives_a_restart`: site list →
durable reservation → revision-fenced refresh → survey unloaded/loaded →
warehouse prepare → materials reservation charged against the player's real
inventory → funded advance committing a stage cell → warehouse bind → server
save/stop → second deployment over the same world with durable receipts proving
exact-once (re-reserve refused operation-conflict). Focused run 3/3 green twice.

**Missing-API findings (recorded, not faked).** The shipped settlement component
cannot express production at a station from warehouse inputs, worker deposit
into the bound warehouse, or food — the plan chain's production/warehouse/food
legs need real surface work (resident-Craft wiring into the settlement
component or new settlement operations) before CP-014 closes. Staged building
is drivable only through the first portion. «Точные три ревизии» read as the
three durable revision lines (reservation, structure commit, warehouse binding)
the scenario pins across restart.

```yaml
base_tree: 32a50c9c
validation:
  - cargo test -p mc-test-harness --test wasm_settlement_operations: 3 passed (x2)
  - fixture-check naevnb2z/y_gmk878: passed
  - clean-worktree gradle six-module suite against pinned loader: BUILD SUCCESSFUL
next: CI run 35426736944 verdict, then implement the missing production/deposit/food surface for CP-014.
```



