# Archived checkpoint log — part 9 of 17

Era: 09-15/09-16. Chronological checkpoint history moved out of `docs/MEMORY.md` so
the live cursor stays small. **Not startup context** (see `AGENTS.md`); read it when
a question is about this era, not to learn the current state.

## Sections in this part

- Живое состояние
- R0 of the settlements overhaul — vanilla villages as sites (landed 2026-09-15; acceptance deferred by the owner)
- R1-A: worker production becomes real cargo (landed 2026-09-15, committed with the R1-B core in `84de0bf9`)
- R1-B: worker cargo reaches the settlement warehouse (core landed 2026-09-15, committed and pushed; plugin half landed 2026-09-16)
- CP-001 closeout — the plugin half of R1-B, plus the documentation sweep (2026-09-16, tree dirty, no commit authorization)
- CP-002 closeout — the load-sensitive settlement red, and the worker-thread bound (2026-09-16)
- CP-003 closeout — warehouse → worker issue, both halves (2026-09-16, tree dirty, no commit authorization)

---

## Живое состояние

## R0 of the settlements overhaul — vanilla villages as sites (landed 2026-09-15; acceptance deferred by the owner)

**Why.** The owner's three-document spec (`../solaris-default-plugins/docs/settlements/`) makes R0 the first finished outcome: one compatible Loader-required package that adopts a real vanilla village, keeps its people and blocks, and shows a truthful overview and one confirmed warehouse content on all three adapters. Entry point confirmed by the owner, including the wire/schema cutover; no scope menu.

**Contract decision (owner-confirmed).** Core already declares Loader wire **3** and client artifact index **schema 2** (`crates/mc-net/src/loader.rs:11`, `MAX_LOADER_VIEW_MESSAGE_BYTES`, `docs/PLUGINS.md:483`). The `solaris-loader` workspace was staged at protocol 2 / schema 1 and its own `AGENTS.md` froze that. The cutover is therefore *the Loader catching up* to the declared contract, not a new number, and `solaris-loader/AGENTS.md` was updated in the same change rather than contradicted silently.

**What landed.**

- **Loader wire 3 / schema 2** (`solaris-loader`, pushed as `0972926`): `PROTOCOL_VERSION = 3`, `INDEX_SCHEMA = 2`, content kinds `views`/`view_actions`/`world_previews`/`world_selection`, the eight schema-2 widget types with their bounds, the `solaris:loader/view` and `solaris:loader/view_action` wire-3 messages, the protocol-3 sound channel, and the view screen in `loader-platform-common`. Verified by my own forced run (`:loader-core:cleanTest … :forge:test`) — 94 tests, 0 failures, 0 skipped across 27 suites; the earlier all-up-to-date run proved nothing and was superseded.
- **A generated vanilla village is a settlement site** (`crates/mc-net/src/script/storage/settlement_village_sites.rs`): identity minted from world identity + dimension + the generator's start chunk (`village_<x>_<z>_<hash8>`, reversible with digest re-derivation), listed in the same page and cursor space as authored sites, queryable by that id, `provenance = vanilla_village`.
- **Contents come from the materialized world, identity from generation.** POIs are read from the placed blocks (beds by the `minecraft:beds` tag, `minecraft:bell`, and the sixteen job-site blocks pinned from the decompiled 26.1.2 `PoiTypes.bootstrap`; receipt at `.analysis/codex-logs/settlement-village-bridge/poi-types-registration.txt`), occupancy from the handles residents hold. Inhabitants come from the chunk's stored `SettlementInhabitantMarker`s, and the entity identity published is the one the spawn lane mints from the placement's claim. A village whose chunks are not generated reports `contents_known = false` rather than an empty village (`ACC-05`).
- **Adoption is the existing `claim_resident`.** `a_generated_inhabitant_is_adopted_by_the_identity_its_placement_mints` proves the chain: marker → spawned villager → descriptor identity → `claim_resident` → one resident; the same operation replays to the same resident, and reopening the storage keeps exactly one. This is the first coverage `claim_resident` has had at all.

**Deliberately not done.**

- **No binding path for a container generation placed inside a village.** R0's evidence is "одним подтверждённым содержимым склада", and that warehouse is the settlement's own authored one (`structures/warehouse.toml`), bound and read through the single existing `bind_warehouse`/`resolve_warehouse_container` path — already covered by tests that read real container contents and refuse unknown/foreign/unloaded handles. A synthetic `DurableStructure` over generated chunks would be a second binding authority that removes nothing (spec §5 certificate semantics, `AGENTS.md` "no duplicate authorities").
- **No removal of the core-village suppression branch.** `main.rs` only suppresses core villages for a plugin that declares a `[worldgen]` settlement plan; the shipped package declares none, so `ACC-01` already holds and editing that branch would be speculative work on a path nothing reaches.
- **No persistent registry of generated villages.** The plan lookup plus live chunk state already answer; a registry would drift on unload, restart and regeneration.

**Recorded gaps (not R0).** No script inventory endpoint reaches an arbitrary world container, so "community starting stock out of the village's own chests" (spec §4.1) has no API yet — that belongs with R1's hauling work, not with the warehouse binding. Village work POIs are classified from the full vanilla registration while the resident lane can still model only `none`/`nitwit`/`toolsmith`; the two limits are separate.

**The client half landed.**

- **The shipped package is Loader-required.** `solaris-settlements/plugin.toml` declares one schema-2 `[client]` bundle (`client/settlements-ui.zip`, 2116 bytes, sha256 `985a8386…`) with `views`/`view_actions` content and `present_views`/`send_view_actions`; the artifact is a deterministic ZIP whose first entry is a schema-2 index declaring one `settlement` screen `solaris-settlements:overview` (a `paged_table`, a `resource_panel` and `refresh`/`page_next`/`page_prev` buttons). The screen shows only what the package holds or reads: settlement identity, the resident roster, the cycle's stop reason and gate needs, and the contents of the warehouse container core bound for it.
- **The server now routes key-driven view requests in production.** `LoaderManifest::from_script_bundles` reads each views bundle's verified artifact index, the manifest exposes `declared_view_kinds()`, and `BoundServer::bind_internal` declares them into the session registry, so `view_request{settlement}` resolves to the plugin that ships that screen. Before this, `declare_loader_view_kinds` was dead code with no production caller and the whole view surface was unreachable. A views bundle with no screen, an unknown screen kind, or a screen id another owner owns now fails at startup (`view_index_routing_fails_closed_on_unroutable_screens`), and the shipped package routes to `solaris-settlements` (`the_shipped_settlement_package_routes_its_declared_view_kind`).
- **The artifact activates under the real Loader.** `solaris-loader` gained `LoaderShippedSettlementsTest`, which activates the shipped bytes through `LoaderContentArchive` and pins the screen kind, title, table, panel and the three declared action ids. It skips when the plugin checkout is absent, keeping the Loader workspace self-contained.
- **Test deployment helpers now copy whole packages** (`settlement_lifecycle.rs`, `mc-server`'s `deploy_sibling_plugin`) instead of a fixed file list, so a shipped artifact cannot silently fall out of a gate.

**Acceptance: deferred by the owner.** The R0 evidence runs (`ACC-01…06`, `CLIENT-01…03`, `REC-01`) are explicitly postponed ("приёмку через харнесс позже проведём"); what exists instead is the code-level half — the Loader activation test above, the core routing tests, and the green gates below. The real-client profiles additionally need `SOLARIS_CLIENT_JAR` and client credentials this machine does not have. **Nothing in R0 is claimed as client-verified.**

## R1-A: worker production becomes real cargo (landed 2026-09-15, committed with the R1-B core in `84de0bf9`)

**Why.** R0's overview can only be truthful about a warehouse if something really puts items there. The read-only R1 map showed the first blocking defect: `ItemLedger::extend_drops` only summed a per-item delta for the receipt, so `Harvest`, `CutTree`, `Mine` and `Fish` removed real blocks, rolled real canonical loot, and left it owned by nobody. Every later step of R1 — haul, warehouse stock, food, construction consumption — had nothing real to move.

**What landed.**

- **Capacity is decided before the world changes.** `ResidentWorld` gained `preview_break`, which computes the *same* canonical loot as the commit without touching the world (loot is a pure function of state, tool and the block's own seed), and `LiveResidentWorld` now shares one `break_loot` between preview and commit so the two can never disagree. The work loop previews, asks `drops_fit`, and only then breaks the block: a worker that cannot hold the loot leaves the field, tree or ore standing and reports `no_storage` instead of producing items nobody owns.
- **One cargo, real stack semantics.** `put_resident_item` now takes the item's own max stack size, merges only into *compatible* stacks (same id, no damage, no enchantments, no custom name or model) and fills fresh slots when merging cannot absorb the rest, so a deposit can no longer create an illegal stack or absorb loot into a tool. `deposit_drops` records in the receipt exactly what entered the cargo.
- **`Fish` was minting cod.** It added `RESIDENT_FISHING_CATCH` straight to the receipt per water column with no owner; the catch now goes through the same capacity check and cargo deposit, and the fixed canonical catch (rather than a loot roll) stays a recorded limitation.
- **A namespaced tool resolved to nothing.** `LiveResidentWorld::item_stack` built `minecraft:{item}`, so a work order naming `minecraft:iron_pickaxe` — the only form the Lua contract accepts — looked up `minecraft:minecraft:iron_pickaxe` and yielded no held tool. Tool-gated blocks then dropped nothing while the worker still counted the work unit: the resident mined ore and produced an empty receipt. `item_stack` now accepts a resource id or a bare path, and the new mine test fails without the fix.

**Evidence.** `harvest_deposits_the_real_crop_into_the_worker_cargo` (cargo holds the crop, the receipt never claims more than the cargo holds, and the same stacks come back through `query_owned_inventory{resident_carry}`); `a_full_worker_reports_no_storage_and_leaves_the_crop_standing` (pause `no_storage`, zero units, no positive change, crop still in the world); `mined_ore_reaches_the_worker_cargo`. Suites: `mc-net --lib resident` 57/0, `settlement` 68/0, `play::` 1789/0.

**Recorded gaps.** The block edit is still committed before the resident record batch, so blocks and receipt are not yet one journal decision (`REC-03`); that composite is the next checkpoint and reuses `WorldInventoryCommit`/`commit_owned_decision`. Fishing yields a fixed canonical catch, not a vanilla loot roll. (The category-like tool ids this section used to record — `minecraft:hoe`/`axe`/`pickaxe`, which are not items — were replaced with real items by CP-001; see the closeout below. The jobs still pause `missing_tool` because nothing issues a tool *to* a worker yet.)

**Checkpoint state (no commit authorization).** base_tree `78b4f39d`; diff_hash
`60072da87f479bddf130c500514127141d7c8f3dcd4be2f7b194fec6e1185e31` (SHA-256 over
`git diff -- crates/` plus the two untracked new files); 48 modified + 2 new
files under `crates/`, `+3920/-261` tracked. Sibling state, which this checkpoint depends on and which is **uncommitted**:
`../solaris-default-plugins/solaris-settlements` — `main.lua` +596 lines (6542
total), `plugin.toml` +13 lines (the `[client]` schema-2 block), `config.toml`
+4/-4, `README.md` +46, and the new `client/settlements-ui.zip` (2116 bytes,
sha256 `985a8386382e37b032533c59b4080843bfe948dc810d2c317106226aefc305a3`,
which `plugin.toml` declares and core verifies at startup). A clean or checkout
of that repository would silently drop the package this checkpoint's core
changes are verified against; the package must be preserved and deployed at the
same revision. Nothing staged, committed or pushed in either repository.

**Gate state at this checkpoint.** `correctness` was run four times; it is **red**, and the red is one test, not the change:

| run | profile | result |
| --- | --- | --- |
| `20260915T061232-correctness-f49zubwq` | correctness | failed — `mc-server --test play`: `lua_script_oversized_payload_is_rejected_before_the_wire` |
| `20260915T061813-correctness-r9tzyw9r` | correctness | failed — same test |
| `20260915T062359-correctness-5wii0zp2` | correctness | failed — `mc-test-harness --test commands`: three Lua chat waits |
| `20260915T063303-correctness-_r712u9v`, `20260915T063737-correctness-z9kkwpzp` | correctness | failed — `mc-test-harness --test settlement_pause_repro`: `settlement_fund_reserves_materials_and_answers_the_player`, 33 s, chat `["Settlements are still loading."]` |

Every failure is a load-sensitive wait that passes standalone: `mc-server --test play` 19/19 (0.67 s), `mc-test-harness --test commands` 13/13 (6.9 s), `settlement_pause_repro` 1/1 twice (3.5 s). This is the family already recorded above (load-sensitive, standalone-green). Two things were tried and **reverted**, because neither converged: widening the per-file wait bounds (`play.rs`, `commands.rs`, `plugin_standard_pack.rs` — reverted in full, `git diff` clean for those files) and `nice -n 10`, which additionally starves the Lua host's own wall-clock slice (`HOST_PLUGIN_MAX_WALL_SLICE` 10 ms inside a 50 ms event budget, `crates/mc-script/src/lua.rs:53`).

The one new fact worth keeping, **measured** rather than sampled: under a controlled load (four `yes` spin clients) this scenario fails 4/4 runs with the current tree **and 4/4 runs with the pre-change package** (`git show HEAD:` copies of `main.lua` + `plugin.toml`, restored afterwards and verified: 6542 lines, `[client]` present, artifact sha256 `985a8386…`); at rest both arms pass in ~3.5 s. So the stall is pre-existing load sensitivity of this scenario, and this checkpoint's plugin change is **not** the trigger — an earlier single-sample A/B that suggested otherwise was noise and is retracted here. Recipe: `for i in 1 2 3 4; do (yes >/dev/null &); done; cargo test -p mc-test-harness --test settlement_pause_repro; pkill yes`. No further bound edits and no plugin-side change for it. It stays an open item: the window is a fixed 30 s while loaded runs take 33 s, so either the scenario's readiness wait or the plugin boot path under contention deserves the fix.

## R1-B: worker cargo reaches the settlement warehouse (core landed 2026-09-15, committed and pushed; plugin half landed 2026-09-16)

**Outcome reached in core.** A `Haul` work order whose destination is a bound warehouse container really moves the worker's cargo into that container in one durable decision; a container that cannot take the cargo leaves every item with the worker and reports `no_storage`.

**What landed.**

- **The chest composite's actor is optional.** `SimulationCommand::CommitChest` now carries `actor_session: Option<SessionId>` and `player: Option<Box<ContainerPlayerPlan>>`; a deposit with no player participant is enqueued with `enqueue_with_fence(None, …)`, exactly like every other server-owned command, and the validator refuses the session-authored menu shape when its session or player plan is missing. `WarehouseTransferRequest` gained `player: Option<WarehousePlayerParticipant>` (one shape with an optional participant, not an actor enum).
- **One container half, factored.** `commit_container_half` (`crates/mc-net/src/play/session/transactions.rs`) owns the state-id fence → `commit_chests_conditionally` → state-id bump → `ChestSlots` publication for both the menu and the server-owned path; only the excluded actor and the refusal family stay caller-specific. The server-owned path keeps publishing to *every* viewer including the actor (verified by the pre-existing `server_owned_warehouse_deposit_appends_before_publishing_and_recovers_both`).
- **The worker's record rides the receipt.** `PreparedStorageBatch::validate_inventory_participant` now accepts a decision whose participants are the receipt and a non-empty `order` change, so the container's after-image and the worker's canonical record are durable together and replay together through the same `append_inventory_projection` / journal recovery the player's after-image uses.
- **Planning is canonical and all-or-nothing.** `plan_warehouse_deposit` (`crates/mc-net/src/play/owned_inventory.rs`) applies every step through `plan_owned_item_transfers`, so a deposit can never state a stack the planner would refuse; a container with no room answers `capacity`, an empty worker answers `insufficient_items`, and the two map to `no_storage` / `missing_input` respectively.
- **A haul is directed again.** `ScriptResidentWorkOrder::canonicalize` swapped `Haul { source, destination }` whenever `source > destination` in enum order (`PlayerInventory < Warehouse < ResidentEquipment < ResidentCarry`), which silently inverted `carry → warehouse` deposits *and* resident→resident hauls. The swap is deleted (`crates/mc-script/src/resident_order_operations.rs`); the fingerprint stays stable because both endpoints are named in the contract. The `Haul` arm also returned its step's move count instead of `already_done + moved`, reporting a watermark that could go backwards on resume; it is now consistent with every other work arm.
- **The plugin half landed 2026-09-16** (`../solaris-default-plugins/solaris-settlements`, uncommitted). `hauling` now binds the committed `solaris:warehouse` container through one shared `S.bind_warehouse` helper (the overview used the same call site before, so the operation id, the core-minted handle, and the replay behaviour are one implementation, not two), carries that core-minted handle into the work order's `destination`, and fails closed: a refused bind abandons the assignment and says so instead of sending a guessed destination. `JOB_WORK` names real items (`minecraft:iron_hoe`/`iron_axe`/`iron_pickaxe`) instead of the categories `minecraft:hoe`/`axe`/`pickaxe` that `gear_has` could never match. **No worker can hold those tools yet** — nothing issues items *from* the warehouse (that is CP-003) — so farm/forestry/mining still pause as `missing_tool`, now for the honest reason.

**Review follow-ups closed in the same checkpoint.**

- **One deposit tail.** `InventoryRuntime::commit_prepared_deposit` now owns everything after a caller has prepared its batch (encode the receipt → commit the container → project the ledger frame → acknowledge the decision); the player's deposit and the worker's deposit share it, so the append/projection ordering ADR 0004 exists to protect has one implementation, not two.
- **Participant pairing is enforced.** The validator admits a chest command only when `actor_session.is_some() == player.is_some()`, and `commit_server_owned` asserts the same against the durable state it holds: a plan that moves a player's items can no longer be committed with the player fence silently skipped.
- **A covered stack no longer stalls the haul.** `plan_warehouse_deposit` advances to the next source slot when the container cannot take the current stack, instead of ending the plan: a worker's cargo whose first stack fits nowhere but whose later stack does deposits the later one. Only a cargo that fits nowhere reports `capacity`/`no_storage`. Guarded by `a_haul_deposits_past_a_stack_the_container_refuses`, which fails against the previous `break` (verified by reverting the one token and rerunning the test).

**Evidence (all green on this tree).** `cargo test -p mc-net --lib` 2206/0; `cargo test -p mc-script` 129/0; `harness run fmt` PASS; `harness run code-health` PASS. New coverage: `worker_haul_deposits_its_cargo_into_the_bound_warehouse` (container merged to the exact count, carry emptied, one decision carrying the receipt, read back through a warehouse query), `a_full_container_leaves_the_cargo_with_the_worker` (no storage, cargo intact, no decision spent, world never asked), `a_replayed_haul_deposits_once` (`REC-02` shape: one real move, one decision), `a_haul_deposits_past_a_stack_the_container_refuses` (mixed cargo progresses past a stack the container refuses). The L2 `correctness` profile was NOT run this checkpoint; the pre-existing load-sensitive red it is known for (`mc-test-harness --test settlement_pause_repro`) is unchanged and unexplained by this work.

**Still open.** Warehouse *withdrawal* by a worker (the same principal in the other direction, CP-003); reserved-stock withdrawal for construction (BUILD-02/03); the construction composite (`REC-03`).

## CP-001 closeout — the plugin half of R1-B, plus the documentation sweep (2026-09-16, tree dirty, no commit authorization)

**Outcome.** `solaris-settlements` assigns a haul whose destination is the settlement's own bound warehouse container, and its job table names real items.

**Evidence.** Two integration tests in `crates/mc-test-harness/tests/settlement_lifecycle.rs` drive the shipped package through its own durable state machine with a scripted core (create → adopt → survey → project → fund → four one-unit build stages → authoritative committed status → populate/spawn → `job hauling`):

- `hauling_work_names_the_bound_warehouse_container` — asserts the admitted `BindWarehouse` comes first and names the committed structure and authored ordinal `0`, then that the admitted `AssignWork` equals `Haul { source: ResidentCarry { handle }, destination: Warehouse { handle } }` with `handle == warehouse_handle(PLUGIN, structure_id, 0)`, fenced on the revision the package just read. Its falsifiability was checked by hand: flipping the package's destination back to `resident_equipment` fails the test, and `main.lua` was restored and re-verified.
- `refused_haul_bind_assigns_no_work_and_reports_the_refusal` — an `unloaded` bind answers with the exact chat refusal and queues nothing: the next admitted command is the reply to a fresh `/settlement buildings`, so no assignment can be sitting behind it.

Gates on this tree: `cargo test -p mc-test-harness --test settlement_lifecycle` 9/9; `harness run fmt` PASS (`20260916T020935-fmt-pmxqzir0`); `harness run code-health` PASS (`20260916T020939-code-health-sxkk4xyt`) — all three re-run on the final tree after the review fixes. L2 `correctness` was **not** run — the known load-sensitive red below is unchanged by this work.

**Not reached, and why.** The plan's full acceptance (`добыча → carry → назначенная доставка → наблюдаемый chest` on a real server) needs a worker that can hold cargo, and nothing issues items *from* a warehouse yet: farm/forestry/mining pause `missing_tool` because no path equips a civilian worker, and a hired soldier is refused a civilian job. That chain belongs to CP-003. This checkpoint proves the order core receives and how the package fails closed, not a filled chest.

**Documentation sweep in the same checkpoint.** `docs/` root now holds current documents only: the two campaign archives (`docs/spark-team/` 87 files, `docs/superpowers/` 56 files) and nine superseded reviews/specs/plans were deleted, milestone sub-documents and the closed alpha plans moved under `docs/milestones/`, the field-test handoff under `docs/evidence/`, and every reference rewritten — a link check over `docs/**` plus the root documents reports 0 broken local links. Stale claims were fixed against code, not against memory: README (worldgen revision 24, the deferred owner acceptance, the Loader wire-3 contract, a documentation map), `CONTRIBUTING.md`, `example.toml` and the `SettlementProfile` docs (the vanilla profile generates villages now), `OPERATING.md`'s worldgen revision, `SOLARIS_LOADER.md` and `ARCHITECTURE.md` wire numbers, ADR 0010's status line, machine-specific paths in `AGENT_TOOLING.md`, and the accidentally tracked `sarvar/` runtime logs plus the superseded `REVIEW_FEEDBACK.md`.

**Checkpoint state (no commit authorization).** base_tree `b676669c`; core `git status`: 165 deletions and 46 modifications (143 of the deletions are the two archive trees), 11 documents re-homed as untracked files; `diff_hash` `53ee06e0f2fb42367966bc77a14925251bd02e692ca1aa5535989968c9792fa9` (SHA-256 over `git diff` excluding this cursor file itself, plus the untracked file contents). Sibling `../solaris-default-plugins`: `solaris-settlements/main.lua` +70/−21 and `README.md`, uncommitted, at base `08e4c3d`. Nothing staged, committed, or pushed in either repository.

## CP-002 closeout — the load-sensitive settlement red, and the worker-thread bound (2026-09-16)

**The red reproduced, and it was not "slow ready".** Under the gate
(`python3 -m tools.harness run correctness`, every test binary at once) the
scenario failed twice at its **first** command with `["Unknown command"]`
(`20260916T035934`, `20260916T042704`): the vanilla dispatcher answered because
the `settlement` root was not routed to the plugin. The root exists before the
server binds — `start_prepared_lua_host` blocks on the host's startup report,
which is sent after the loop that calls `register_plugin_routes` — so it can
only disappear through `unregister_plugin_routes`, i.e. the host's
"Lua plugin disabled after handler failure" path (`crates/mc-script/src/lua.rs:2856`),
or the authority's permanent `clear()` on shutdown. The host cuts an invocation
off on a **wall-clock** slice (10 ms per plugin in a 50 ms event budget), which a
descheduled thread trips exactly like a script that really burns 10 ms.

**Why it was reachable.** One scenario process peaked at **18 threads**
(sampled `/proc/<pid>/task`). Note for the record: `cargo test --workspace
--all-targets` runs test *binaries* one at a time (every `Running tests/...`
block in `20260916T042704-correctness-epbdmcnp/test.log` is followed by its own
`test result` before the next one starts), so the failure is not cross-binary
contention - it happened with the rest of the machine idle, which points at
process-internal stalls (allocation, page faults) or a borderline 10 ms slice
rather than at a dozen concurrent servers.

**The fix (owner: the process's worker budget).** `[chunk_pipeline] worker_threads`
is an absolute bound — never a percentage, never above the derived default unless
the operator says so: it caps the shared chunk/entity CPU pool and the chunk IO
pool, the region owner lanes (they already size from the same `cpu_capacity()`),
and the startup bake (`startup_chunk_worker_threads` returned
`max(configured, available)` before, so a configured bound was silently raised to
the CPU count). `playable.toml` now sets `worker_threads = 2`, `example.toml`
documents the key, and in-process servers in `mc-server`'s play/configuration
tests and in the settlement scenario use `ChunkPipelinePolicy::bounded(2)`.
Measured: that scenario process now peaks at **9 threads**; a temporary probe
(removed) showed no host invocation above 5 ms at rest or under 16 spinners, so
the slice was lost to scheduling, not to script work.

**Evidence.** `correctness` PASS three times in a row on this tree
(`20260916T043818` 223.3 s, `20260916T044201` 221.6 s, `20260916T044543` 224.2 s)
against the same gate that failed twice before. Focused: `mc-server --lib` 77/0,
`--bin mc-server` 67/0, `--test play` 19/0, `--test configuration` 14/0,
`settlement_lifecycle` 9/0, `settlement_pause_repro` 1/0, plus the new
`chunk_worker_threads_bound_replaces_the_derived_split` and the extended
`startup_chunk_workers_cover_configured_and_available_parallelism`. Receipts and
the full reasoning: `.analysis/codex-logs/settlement-readiness-stall/README.md`.

**Residual, named.** The host's budget metric is unchanged: an invocation that
trips the wall slice still disables the plugin. The bound removes the condition
that made it reachable here, and the scenario now installs a `warn` subscriber so
a future timeout carries the host's own reason instead of a chat dump alone. If
the red returns, the metric is the next owner.

## CP-003 closeout — warehouse → worker issue, both halves (2026-09-16, tree dirty, no commit authorization)

**Outcome.** A settlement can issue a *named* item out of its own bound warehouse
container into a worker's own endpoint, in core and in the shipped package, and
the assignment reports what left the container.

**Core.** A `Haul` order is now directed by both endpoints and carries an optional
`item`:

- `ScriptResidentWorkOrder::Haul` gained `item: Option<String>`, validated as a
  contract resource id (`crates/mc-script/src/resident_order_operations.rs`), and
  the Lua parser accepts `item` next to `kind`/`source`/`destination`
  (`crates/mc-script/src/lua/operations.rs`).
- `plan_warehouse_deposit` (`crates/mc-net/src/play/owned_inventory.rs`) takes the
  filter and skips stacks it did not ask for; its two sides are now
  `updated_source`/`updated_destination` because a move runs either way. A
  filtered move that matches nothing answers `insufficient_items` (a missing
  input, made exact by the filter) while a destination that cannot take a stack
  the source really holds answers `capacity` (no storage).
- `plan_resident_warehouse_move` (was `plan_resident_warehouse_deposit`,
  `crates/mc-net/src/script/storage/world_inventory.rs`) accepts either direction,
  validates the resident handle against the record, and writes the planned
  after-image back into whichever of `carry`/`equipment` the worker's endpoint is.
- The `Haul` executor arm stages the container composite when a Warehouse is on
  **either** side (`stage_warehouse_move`, was `stage_warehouse_haul`) and maps
  refusals honestly: an unresolvable or refusing container is `no_storage`, a
  full container is `no_storage`, a worker whose own endpoint has no room is
  `interrupted` (its room can be freed by the opposite move), and an item the
  container does not hold is `missing_input`.
- The assignment's `changes` are signed for the container a haul moved through:
  a deposit reports what entered it, a withdrawal what left it
  (`ResidentWarehouseMove::container_delta`).
- A resident→resident haul honors the same filter (`haul_resident_items`), so
  `item` never silently means something different by direction.

**Package** (`../solaris-default-plugins/solaris-settlements`, uncommitted). The
refusing `deposit` stub is deleted and replaced by
`/settlement issue <name> <resident> <item> <count> [equipment|carry]`: it
validates the item and the batch (1–4096, core's `MAX_WORK_UNITS`), reads the
resident back from durable storage, resolves the settlement's committed
`solaris:warehouse` through the same `S.bind_warehouse` helper the hauling job
uses, and assigns `Haul { source: Warehouse{handle}, destination: resident_equipment
| resident_carry, item }` with the stated count. The bind is idempotent (one
deterministic operation id per settlement and structure), a refused bind assigns
nothing, and the durable intent is parked (`issue-intent`) before the call so a
restart recovers the committed receipt instead of issuing twice.

**Evidence.** Core, in `crates/mc-net/src/script/storage/resident_settlement_tests.rs`
(5 new, all against the real storage/journal/deposit harness): a withdrawal takes
the named item even when it sits behind another stack (receipt `delta = -2`, one
journal decision carrying the record and the container, warehouse query reads the
remainder back); the same into equipment; an absent item pauses `missing_input`
with no decision spent; a full carry pauses `interrupted` and leaves the container
untouched; an unknown handle pauses `no_storage` and stays durably paused; a
replay takes the item once. `resident_order_tests.rs` adds the filtered
resident→resident case and its absent-item case. `mc-script`'s DTO test now covers
`item` acceptance and rejection (bad ids, empty id, identical endpoints).
Package-side, `crates/mc-test-harness/tests/settlement_lifecycle.rs` drives the
shipped `main.lua` through its own state machine: the admitted `AssignWork` equals
`Haul { source: Warehouse { warehouse_handle(PLUGIN, structure_id, 0) },
destination: ResidentEquipment { handle }, item: Some("minecraft:iron_hoe") }`
with `work_units = 2`, the carry variant reads and fills the carry endpoint, and a
refused bind assigns nothing. Falsified by hand both ways: `item = nil` in the
package fails the two issue tests, and swapping the planner's sides fails all four
withdrawal tests; both files were restored and re-verified green afterwards.

**Not reached.** The plan's full R1 acceptance (`добыча → carry → назначенная
доставка → наблюдаемый chest`, and now `issue → worker really uses the tool`)
still needs a *world* source of settlement stock: the warehouse container in
these tests is filled by the fixture, and a village's starting stock has no
container owner yet. That is the next item of the plan's CP-003 list ("закрыть
пробел источника world-container/village-stock через того же владельца
контейнера"), plus the CP-004-onward checkpoints.

**Gate state.** L2 `correctness` **PASS**
(`20260916T051759-correctness-z1ry_tg2`, 382.4 s; all four commands exit `0`:
`fmt --check`, workspace `clippy -D warnings`, `code-health`, `cargo test
--workspace --all-targets`), on the tree *before* the review fixes below. One
earlier run of this gate failed and is recorded here rather than dropped:
`20260916T051715-correctness-e18e3isk` **failed in 24 s on clippy** - the new
`item` parameter pushed `plan_warehouse_deposit` and
`plan_resident_warehouse_move` past clippy's argument limit and left
`seed_equipment` unused, all three fixed before the passing run. L1 on the same
tree: `fmt` PASS `20260916T052448-fmt-_2v26lhp`, `code-health` PASS
`20260916T052453-code-health-m4t65g3y`.

**Coverage argument for the plan's remaining CP-003 checks.** The plan also asks
for a concurrent player click, a stale container binding, open viewers seeing
the accepted version, and a real storage reopen. Those live in the *shared*
container half: a resident warehouse move and a player deposit both commit
through `commit_prepared_deposit` → `commit_container_half` →
`commit_chests_conditionally`, one function, with the same state-id fence,
publication to every viewer and journal recovery. That half is already covered by
`server_owned_warehouse_deposit_appends_before_publishing_and_recovers_both`
(`crates/mc-net/src/play/simulation.rs`) and
`warehouse_transfer_refuses_foreign_unknown_unloaded_and_stale_containers`
(`settlement_tests.rs`); what is direction-specific - which side is the resident,
the signed receipt, the after-image written back into `carry` vs `equipment` - is
what the new tests pin. The one case the work path adds on top is a stale
*resident* revision, refused by `resident_order_execution.rs:227` before any
planning, which is direction-agnostic.

**Checkpoint state (no commit authorization).** base_tree `b676669c02bcb4b4d5d886aa6f3b19ed2b072f17`;
`diff_hash` `f3729e60e8cfc5b30389e67692c503648cb7380f72201d0914f97f8432a50b47`
(SHA-256 over `git diff` with this cursor file excluded, plus the 11 untracked
documents' contents). Core files this checkpoint touched:
`crates/mc-script/src/resident_order_operations.rs` (+`Haul::item`),
`crates/mc-script/src/lua/operations.rs` (parser + import),
`crates/mc-script/src/resident_order_operations_tests.rs`,
`crates/mc-net/src/play/owned_inventory.rs` (item filter, renamed plan fields),
`crates/mc-net/src/script/storage/world_inventory.rs` (both-direction planner,
signed receipt), `crates/mc-net/src/script/storage/resident_order_execution.rs`
(staged move on either side, filter), and the two core test suites plus
`crates/mc-test-harness/tests/settlement_lifecycle.rs`. Sibling
`../solaris-default-plugins` at base `08e4c3d`: `solaris-settlements/main.lua`
+296/−34 and `README.md` +35/−…, uncommitted. Nothing staged, committed or pushed
in either repository.

