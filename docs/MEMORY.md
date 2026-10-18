# Solaris current cursor

Живой курсор проекта. Здесь только то, что верно **сейчас**, и карта остального. Историю
держим в [`docs/memory/`](memory/) — это **не** стартовый контекст (`AGENTS.md`); открывай
часть, когда вопрос про ту эпоху.

## Core v2 parity program 2026-10-11 +0700 (ACTIVE)
Full live handoff (owner decisions, order, host/agent rules, next steps): `.analysis/codex-logs/parity/HANDOFF.md` — read it first on resume.
- Owner: measure only a finished v2. Before perf: OLD->v2 functional parity (`.analysis/codex-logs/parity/old-vs-v2-matrix.md`), wave 2 incl. Nether/End/portals/dragon, wave 3 catalog mechanics absent in both cores (sculk/warden, trial chambers, crafter, copper golem, bees, allay, raids, mace, new effects, shelves, Golden Dandelion), native compact storage (extend `mc-world/src/native.rs`; Anvil import/export only), passing real-client scenarios for new surfaces.
- Perf follows the owner's load catalog `.analysis/codex-logs/parity/SOLARIS_MINECRAFT_LOAD_CATALOG.{md,json}`; resume contract `.analysis/codex-logs/coreperf-20261011/window-hold.yaml`.
- Vanilla exactness = packet/world layouts only; gameplay numbers free, no copying vanilla bugs.
- State: wave 1 CLOSED green (6727 pass). Wave 2 CLOSED green 2026-10-12 (6932 pass, receipt `.analysis/codex-logs/parity-w2-gate/correctness-receipt.json`): special mobs, mounts, vehicles, partials, Nether/End D1-D9, vanilla 26.1 world layout. Wave 3 CLOSED green 2026-10-12: sculk/vibrations/warden, trial spawner/vault/omens/wind charge, mace + 4 effects, crafter/shelves/golden dandelion, copper chests/golem/statues, bees/allay/jukebox, raids/Hero, conduit/dolphin, sign/flower support pop, dropsLike + copy_components/copy_state, item component wire codec, corrupt-chunk fail-closed loader. W3 gate fingerprint ed84557d…8c25: vanilla 26.1.2 roundtrip PASS `.analysis/vanilla-world-roundtrip/w3-resume-20261012T115533/result.json`, L2 PASS 7140/0/192, harness-check PASS, receipt `.analysis/codex-logs/parity-w3-gate/final-correctness-receipt.json`; frozen binary `.analysis/codex-logs/client-qa/bin/solaris-v2-87ad35b9`. Owner 2026-10-12: structures later; real clients GO (native + Java) — QA running (ClientQAJava/ClientQANative, matrices in `.analysis/codex-logs/client-qa/`). Gaps: no structures, single-biome Nether/End, riders excluded from portals, 82 item components unsupported on wire (fail loudly), copper golem has no sounds; perf debt: redstone per-tick O(containers) comparator scan.
- Real-client QA CLOSED 2026-10-13 (owner GO: native + Java; structures deferred). Fix loop during QA (all in candidate): clock SetTime, bottle fill, fire air-spread + 207-entry FireBlock flammability table, /summon common tags, held-use restart guard, /give components + /clear predicates, leash detach, generic spawn eggs, loom index via sent tag order, merchant stale stateId, restock gate >2400, `mc-world` dev opt-level 3 (debug chunk admission ran ~2-9 TPS; release 20.0 TPS p99 37 ms view 8), ItemStackTemplate codec (charged_projectiles; bundle_contents/use_remainder enabled), canonical BE table (49 kinds/201 blocks) + default BE creation + shulker menu/custody + hoppers, accepted_placement tombstone, setblock same-block retention, embedded arrow/stinger counts, item motion + Q throw, item hazards, POV OUTLINE ray (`block_outline_shapes_26_1_2.bin`), unknown item id rejection, bundle cap 64. Independent review `ServerFixReview` (1 P1 + 9 P2) all fixed. Final candidate fingerprint 3ba9c13a…0998: L2 PASS 7316/0/192 `.analysis/validation/20261012T214016-correctness-89wnjspa`, vanilla roundtrip PASS 18 checks `.analysis/vanilla-world-roundtrip/final-20261012T220527`, native matrix 14 PASS/1 client gap (flint) `.analysis/codex-logs/client-qa/ClientQANative-matrix.md`, Java matrix 33 PASS/6 BLOCKED (legacy 21/22/25/26/28/29 terrain/approach)/0 FAIL `.analysis/codex-logs/client-qa/ClientQAJava-matrix.md`. Timing-sensitive real-client checks run on release. Open (next batch): merchant title "Villager" hardcoded (tick/merchant.rs:170), `!damage_resistant` removal ignored by item hazards. Next: native storage slice (1d).
- Native storage + Anvil converter + old worlds + load harness CLOSED 2026-10-13 (uncommitted): runtime persists only native redb (`mc-world/src/native*`) + tick WAL, no runtime Anvil/JSON manifest; recovery = checkpoint + contiguous WAL from checkpoint+1 (hole/overlap refuses start); entity payload UUID must match record key; one checkpoint in flight (`/save-all` waits on `checkpoint_rx`). Offline `solaris-v2 world import-anvil|export-anvil` (lossless: unknown tags + saved data verbatim, signed overdue tick deltas, exact `flowing_*` fluid IDs, `SolarisJournalLsn` stripped, export refuses unclean DB). Old worlds DataVersion 2860..=4790 via `mc_world::anvil::upgrade` (DFU inventory `.analysis/old-world-upgrade/fix-inventory.md`); legacy saved data/POI kept byte-exact at modern paths per javap `DimensionStorageFileFix` (raids: only `raids.dat`/`raids_end.dat`; vanilla DFU upgrades on load). Load catalog harness `tools/harness/backends/catalog_load.py` + TickTimes wall telemetry; smokes only, `performance_evidence=false`. Gates: L2 PASS 7569/0/192 `.analysis/validation/20261013T162950-correctness-h0vodaxj`; vanilla 26.1.2 roundtrip r6 PASS 18/18 `.analysis/vanilla-world-roundtrip/native-cutover-20261013-r6/result.json`; native graphical (Xvfb `--windowed`) `survival` v2 PASS with screenshots `.analysis/validation/20261013T170307-survival-o8ulxrme`. After those gates only the non-vanilla `raids_nether.dat` alias was removed (converter tests 27/0). Review `.analysis/codex-logs/integration-20261013/review-integration-20261013.md`: F1-F3 fixed. Gaps: fresh-world partial-creation (F4) deferred; Nether non-zero border centre refused; boss events/scheduled functions/stopwatches preserved not simulated. Native client build needs `PKG_CONFIG_PATH`+`LIBRARY_PATH` = `~/solaris-client/.analysis/build-support/usr/lib/x86_64-linux-gnu` (client `PLAYING.md`). Next: P1 parity gaps (`.analysis/codex-logs/parity/old-vs-v2-matrix-reaudit-20261013.md`), redstone dirty-driven perf debt, then exclusive-window load catalog.
- P1 parity batch 2026-10-13 (uncommitted, merged from 4 worktrees): piglin barter (`piglin.rs`: gold ingot -> 120-tick admire -> 19-entry vanilla barter table, gold-armor neutrality, brutes/babies no barter, zoglin/hoglin minimal; barter start/payout/ground-stack debit journaled), armor stand (`armor_stand.rs`: place/equip/swap/take per ShowArms+click height, javap drop rules: player break = stand+all equipment, other death = equipment w/o vanishing, creative//kill/void = nothing; name tag/marker pass through; `PlayerInput::InteractEntity{,Secondary}.location`), snow golem + wandering trader (`mob_lanes.rs`: snowballs, trails; trader 1200-tick counter, 24000 delay, 25/50/75 %, 2 leashed llamas, 48000 despawn, 9 vanilla offers, saved delay/chance stored losslessly), leaves (`leaves.rs`: O(changed) distance, decay via loot tables, placed = persistent) and signs (`signs.rs`: dye/glow/ink/wax per side). Review `.analysis/codex-logs/integration-20261013/review-p1-parity.md`: 1 P1 (barter WAL) + 3 P3 fixed. Gates: L2 PASS 7615/0/192 `.analysis/validation/20261013T190121-correctness-3m923zoy`; windowed native `survival` v2 PASS `.analysis/validation/20261013T192828-survival-gszqg36d`. Gaps: snow golem not buildable from blocks (no golem-construction path), trader water-bottle offer unpublishable (ItemCost potion predicate), enchanted-pickaxe roll not bit-exact. Not done (ask owner): advancements, player statistics.
- P2 parity batch 2026-10-14 (uncommitted, from ~/solaris-wt-{fishing,mobsa,mobsb}): fishing via vanilla 26.1.2 `gameplay/fishing` tables (`mc-data/src/loot/fishing_26_1_2.rs` include_str from data/vanilla), open-water 5x4x5 treasure gate, Luck of the Sea/Lure, rain bonus via shared `conduit::rain_exposed`; slime/magma linear sizes 1/2/4 with vanilla halving split, journaled children; enderman (`enderman.rs`): projectile immunity (only arrows deflect), water/rain damage + teleport, stare aggro w/ pumpkin; creaking (`creaking.rs`, unbound only: frozen while watched), giant (no AI, 100 hp/50 atk), illusioner bow + invisibility. Review `.analysis/codex-logs/integration-20261013/review-p2-parity.md`: P2 deflection gate fixed, P3 rain scan unified, P3 enderman replay ground=false accepted (physics re-grounds next tick). L2 PASS 7634/0/192 `.analysis/validation/20261013T202332-correctness-wy9s615f`. Gaps: heart-bound creaking (no creaking_heart owner), parity row 67 (old equipment/nav model 1:1) not pursued.
- Post-P2 fixes 2026-10-14 (uncommitted): `/summon slime|magma_cube {Size}` (vanilla clamp 0..=126); equippable UseItem swap per javap `Equippable.swapWithEquipmentSlot` (swappable/allowed_entities retained in `mc-data` item components) + player equipment projected to trackers (never to the owner: client writes MAINHAND into its selected slot); player views used protocol id 128 = `minecraft:stray` → registry `minecraft:player` (`entities::player_type_id`); redstone comparator/trapped-chest signals dirty-driven via custody wrappers (`redstone/custody.rs`, per-tick full scan removed; non-exclusive core-bench `containers` 20k chests p50 40.1 → 2.65 ms, exclusive A/B command in `~/solaris-wt-redstone/.analysis/codex-logs/core-v2-bench/redstone-dirty-result.yaml`). Review `.analysis/codex-logs/integration-20261013/review-equip-redstone.md` (P1 self-equipment fixed, P3 fixed). L2 PASS 7654/0/192 `.analysis/validation/20261013T220659-correctness-y7lv8hn4`. Native client fixes (sibling, uncommitted): sign editor default text + unique line labels, ArmorStandItem placement, pickup reach, armor-stand armor rendering (no humanoid armor layer for players/zombies yet). Real-client QA: `.analysis/codex-logs/client-qa/P1Parity-native.md`, `P2Parity-native.md`.
- Statistics + advancements + load-control server fixes 2026-10-14 (uncommitted, merged from ~/solaris-wt-{stats,adv,stock,skeleton,idlewal,village}, worktrees removed; diffs/receipts `.analysis/archive/worktree-receipts/wt-*`): vanilla statistics (`mc-data/src/statistics.rs`, `mc-core/src/statistics.rs`; 77 custom + 9 types; compact `JournalEntry::Statistics`, time counters derived from tick anchors; REQUEST_STATS -> AwardStats 0x03; Anvil `players/stats/` + legacy `stats/`, stats-only players admitted); advancements (`mc-data/src/advancements*`, `mc-core/src/advancements.rs`, `mc-edge/src/advancements.rs`; 1617 vanilla defs incl. 1492 recipe unlocks loaded from ignored `data/vanilla/data/minecraft/advancement` — extract via `tools/extract-vanilla-data.sh`, mc-core tests need it; Arc COW progress, `None` = unchanged in WAL; grant/revoke; Anvil `players/advancements/`, orphan files kept byte-exact as extras — known minor: such an orphan is dropped on export after that UUID joins natively); `CustomNameVisible` authoritative + metadata 3; RemoveEntities decode uncapped (remaining-bytes bound); hopper suction includes the bowl; private block-entity NBT (hopper cooldown deadline, clamp 0..8 on export) no longer bumps client chunk revisions, edge sends unseen chunks before repair debt; `ClientboundDamageEvent` on fresh hits only; warden warning timers tick-anchored (idle 10 players x 200 ticks WAL 12.2 MB -> 0) + Leave final player image; villager job-site reach sq 9 + work strolls. Gates: L2 PASS 7833/0/201 `.analysis/validation/20261014T143314-correctness-mhyhhe4k`; real-Java Anvil roundtrip 50/50 `.analysis/codex-logs/anvil-roundtrip-20261014/verdict.yaml`; native real-client P4 (nameplates, advancements tabs/persist, hurt tilt, hopper Q, statistics screen) `.analysis/codex-logs/client-qa/P4Parity-native.md`; reviews `.analysis/codex-logs/{merge,post-merge}-review-20261014.md` (findings fixed). Load controls smoke p5 `.analysis/codex-logs/catalog-public-controls-20261014/controls-p5-2115` = MIXED (triage running). v2 plugin host port in progress (`~/solaris-wt-plugins`). Native client sha 2c5ae20b (stats screen, adv tabs, hurt tilt, nameplates).
- v2 plugin host + load fixes 2026-10-15 (uncommitted, worktrees merged/removed; archives `.analysis/archive/worktree-receipts/wt-plugins*`): solaris-v2 loads Wasmtime packages from `[plugins].directory` (`mc-edge/src/plugin_loader.rs`, `mc-core/src/plugin*.rs`, `build_precommit.rs`); plugin KV/receipts/custody in native store + same tick-WAL batch (pre-port stores open with empty plugin domains; advertised-but-missing domains fail closed; receipts unbounded by design — eviction would allow re-execution); guest calls off the tick; `before-build` hooks per owner (v1 surface: place/break/buckets/bone meal/composter/server-owned edits) via edge admission, 100 ms ends at host answer, deny = block+inventory resync, no WAL; observations shed per plugin at 256; unimplemented imports fail closed (damage-entity, client views/selections, loader block items, custom payload; before-damage declarations refused). Settlements needs the Loader client (vanilla clients disconnected in Configuration by design). Per-connection teleport ids + move fence until matching confirm (fixed tp rollback, `.analysis/codex-logs/teleport-rollback/`); villager repath deadline not renewed by revision churn (`.analysis/codex-logs/village-motion/`). Gates: L2 PASS 8134/0/205 `.analysis/validation/20261015T005217-correctness-t9k03lcw`; review `.analysis/codex-logs/plugin-port-review-20261015.md` (pass after fixes); real-Java roundtrip 53/53 incl. pre-port store `.analysis/codex-logs/anvil-roundtrip-20261015/verdict.yaml`; real TCP towns claim/deny + economy crash/WAL exactly-once (`.analysis/codex-logs/plugin-host-v2-rebase/`). Control smoke on release p8 (`solaris-v2-p8-a72ad220`, shared host, not perf evidence) `.analysis/codex-logs/catalog-public-controls-20261014/triage-p11-load-harness.yaml`: PASS explore, built-base, village, projectiles, farms, long-session; PERF_FAIL dense-hostile, spread-passive, fluids, combat, plugin-heavy (login p99 51 ms); CORRECTNESS_FAIL checkpoint-restart (one harness close offered before menu content arrived; restart itself clean). Next: exclusive perf window per `.analysis/codex-logs/coreperf-20261011/window-hold.yaml`.
- Denied placement repair 2026-10-15: towns claims use native zone protection without a `before-build` hook. Hook-free protected/reach refusals now use the same reliable block + acknowledgement + authoritative inventory resync as precommit refusals, rather than block-only global corrections. Frozen p8 guest save retained 8 cobblestone while Java displayed 7 (no item loss). Repro/repair receipts: `.analysis/codex-logs/denied-place-inventory/`; graphical rerun remains Main-owned.
- Cleanup + lock reduction 2026-10-15 (uncommitted; merged TickSplit `tick/input/*`+`tick/damage.rs`, slime/honey pistons, OpsCli `operator_cli`/`content_*`/`backup`/`console` + `command_console`, mob equipment; ADR 0014 v1 retirement phases 1-2: crates `mc-loader`, `mc-vanilla`). Edge: `plan/player_changes.rs` (borrowed per-change planning, dead branches removed), `decode.rs` single `match` on `Packet::ID`, `ClientboundSetCursorItem`; `PlayerChange` lost `connection`/`Chat.recipient` (queue key is the address); `command_execution` player commands split. Locks (audit `.analysis/codex-logs/lock-audit-20261015/summary.md`): tick hands reliable outbox to the edge by mpsc batch (ADR 0013), edge routes from one registry snapshot per pump tick with atomic register/stash join handoff, parking_lot registry/admin/sessions, plugin routes/roster via ArcSwap, worldgen caches compute outside locks; mc-world storage findings are v1-only (delete per ADR 0014). Also dyed_color (component 44) + DyeRecipe crafting, solaris-loader smoke API fix. L2: fmt/clippy/code-health PASS `.analysis/validation/20261015T053039-correctness-n6ull2h2`, test PASS 8270/0/205 `.analysis/validation/20261015T064544-test-pv1ohxkx`. Native windowed inventory v2 PASS `20261015T064341-inventory-ym2ewwu_`; Java towns/armor/cursor round running (P1ClientQA). Perf unmeasured -> evening window.
- v1 retirement + load/perf pass 2026-10-15 (uncommitted). ADR 0014 phases 3 and 5 done: harness runs only `solaris-v2` (player-scale/load-matrix/entity-scale/living-world-scale profiles deleted; `catalog-load` replaces them), crates `mc-server`/`mc-net`/`mc-simulation`/`mc-entity` deleted (-517k lines; diff `.analysis/archive/worktree-receipts/wt-phase5-final.diff.gz`). Sibling `solaris-default-plugins`: v1 integration tests deleted (archive `.analysis/archive/sibling-v1-integration-tests/`), gates `standard-pack`/`settlement-cargo` start `solaris-v2`; standard-pack incl. `--real-client` PASS (`.analysis/sibling-phase5-20261015/`). Core: InhabitedTime accumulation (WAL interval images, Q6 kept; ADR 0013); command `fill`/`setblock` edits visible to same-tick summons (cows no longer suffocate; `.analysis/codex-logs/cow-deaths/`); fresh worlds now generate native villages via the existing `VillagePlanSource` (`.analysis/codex-logs/native-village-20261015/`); tick hot paths (shape updates, tags, light seam baking off tick, checkpoint capture 159->4 ms; `.analysis/codex-logs/tick-perf/`). Test gate = nextest, 750->487 s. Load harness: all MIX + mixed-survival runnable (`.analysis/codex-logs/mix-oracles-20261015/`), mixed mob fixtures (shared OLD/v2 4 passive kinds; v2-only +4 hostile), towns/economy and real settlements-NPC scenarios, bots with Loader client (`.analysis/codex-logs/loadset-20261015/`). Dev profile: third-party deps opt-level 2 (debug startup 64->28 s). Next: frozen candidate -> 12 controls + MIX + new scenarios + OLD/v2 A/B.
- Server memory 2026-10-16: deterministic adult chicken egg countdowns now advance from existing WAL batch ticks, avoiding a fresh 1,216-byte retained state image per chicken per tick; initialization/reset and egg births remain atomic. Replay covers health-only images and repeated folds without changing serialized fields. `mc-core --lib` 1756 PASS/10 ignored, strict all-targets Clippy + 22 coreperf tests PASS. Same public 100k mixed-mob + 64-villager prepare/save/reopen PASS (100,064 exact cohort UUIDs); server VmHWM 2.274 GiB versus 14.937 GiB before failure at ~50k submitted. Receipts/heap attribution: `.analysis/codex-logs/server-memory/`.
- MIX villager WAL/recovery 2026-10-16 (uncommitted): deterministic age/cure/work/sleep/panic clocks now project from durable anchors; event/reset images remain, unloaded/NoAI/dead boundaries and legacy missing clock flags are covered. Recovery streams one decoded batch, folds clocks once, and opens the synchronous core on Tokio's blocking pool. The journal writer inherits exact validated tail coverage under the world lease, avoiding first-checkpoint re-decoding. Before sampled WAL averaged 30.0 MB/tick; final 32-tick sample averaged 20.3 KB/tick (5.875 full entity images/tick, no EntityState). Final cold recovery of a preserved 13.01 GB tail peaked at 1.187 GiB and stopped cleanly in 7.79 s; 100-client private MIX reopen/prefix peaked at 1.165 GiB and stopped in 3.43 s (not a TPS comparison). Core 1769 PASS/10 ignored, strict all-targets Clippy PASS; receipts, limitations and sealed binary: `.analysis/codex-logs/server-memory/mix-villagers/summary.json`.
- L2 failing-snapshot repair 2026-10-16 (uncommitted): exact 64-test filter PASS 64/64 after explicit fixture activity/residency, current simulation-debt clocks, S1-applied checkpoint policy, non-destructive deployment duplicate probes, and self-contained harness provenance/combat evidence. Two production regressions fixed: `avoid_vibration` now respects the sensor listener radius; offline import applies native S1 startup before persisting its identity, so converted worlds reopen under the same policy as `Core::open`. Native-item Loader exemption remained SettlementsS1-owned; importer map counters remained Constantinople-owned. Scoped commands, first-failure receipts, classifications and source identities: `.analysis/codex-logs/l2fix/result.yaml`; this is exact-failure closure, not a fresh full-workspace L2 claim.
- G1/G2 custody perf 2026-10-16 (uncommitted): portal zero-timer scans and persistent idle resets no longer mark unchanged rows; allocator validates villagers in place. Interleaved `-p` frozen/current G2 journal p50/p99 11.679/12.417 → 0.843/1.110 ms (target PASS), touched rows 11,557 → 372 median. Activated moving 50k G1 p99 54.532 → 39.624 ms (5 ms target still missed; remaining allocator/budget p99 25.811 ms and physical continuation p99 12.729 ms, not additive quantiles). Focused 182 PASS/5 ignored and strict mc-core all-targets Clippy PASS. ADR 0013 records boundaries; exact A/B, profiles and remaining costs: `.analysis/codex-logs/perf-g1g2/result.yaml`. Not an exclusive server-load acceptance gate.
- G6 native storage 2026-10-17 (uncommitted): checkpoint chunk preparation now streams one frozen baseline/effective pair into the same atomic native transaction; eager whole-capture baseline/deferred vectors and API removed, all fixture callers migrated. Imported Falcons copy (144,753 chunks) + 5,000 villagers/houses passed save → SIGKILL9 → reopen under enforced 6G shared slot: 145,842 captured chunks, 47,489 entities (42,489 original + 5,000), 5,504 villagers and 5,000 houses/beds/job POIs preserved; child capture/commit VmHWM about 2.01/2.00 GiB. Focused native/core checkpoint 18/152 + loader/world 7/25 PASS; strict mc-world/mc-core all-targets Clippy PASS; independent review clean. Native file grew 4,165,472,256 → 24,650,969,088 bytes; commit 988 s is shared correctness evidence, not a perf window. Receipt `.analysis/codex-logs/g6-storage/many-chunks-result.yaml`; stopped world `.analysis/codex-logs/g6-storage/many-chunks-world` handed to Constantinople for his separate native smoke.
- G6 no-op storage 2026-10-17 (uncommitted): restore distinguishes durable bookkeeping from newer WAL values; block/fluid schedulers keep checkpoint projection dirtiness separate from WAL emission and release it only on matching per-chunk ACK. Unchanged chunk/entity bytes and old row LSNs are retained, with the global checkpoint fencing stale WAL (including absent keys); recovered-only saves need no synthetic append. Exact native write counters are separate from logical live data and file length. Disposable imported copy, 47,491 entities, enforced 6G: both saves captured 0 chunks, rewrote 0 chunks/entities, and wrote 0 such payload bytes; logical data stayed 17,821,590,044 B and file length 23,519,899,648 B. Native 51 + scheduler/fluid 64 PASS; broader core 366 PASS/1 ignored, with its one new fixture WAL-boundary assertion corrected and covered by the final 64; strict mc-world/mc-core all-targets Clippy PASS; four read-only reviews clean. No compactor or performance-window claim. Public startup/2400-tick/RSS acceptance remains Constantinople/Main-owned. Receipt: `.analysis/codex-logs/g6-storage/noop-result.yaml`.
- Native-100 residual tick work 2026-10-17 (uncommitted): placement obstruction uses live entity chunk buckets and a conservative horizontal bound maintained by spawn, dimension transfer, and the sole width setter (all import/growth/type-change callers migrated). Unarmored movement skips physical gear resolution; despawn uses bounded observer existence queries; ages/despawn reuse ordered active-slot admission, retaining global player age and deterministic removal/RNG order. Shared dirty-refreshed R4 entity flags admit only effect-bearing bodies/players and relevant lifecycle clocks before ColdRow access; effects need no candidate allocation, and lifecycle checks activity only for admitted clocks. Abstract catch-up visits only its owed chunk; mount/trader/snow/vehicle and piglin guards reject unrelated bodies before activity lookups. WAL/replay formats and simulation cadence unchanged. Original r1 period-weighted breakdown and exercised focused receipts: `.analysis/codex-logs/opt-tick-rest/result.json` (`serial-tests.log` 63 PASS/1 ignored; `guard-tests.log` 103 PASS; corrected new barter-deadline fixture `piglin-retry.log` 1 PASS; shared R4 focused gate 331 PASS/2 ignored). No native-load/timing acceptance from this slice; inferred savings are not measurements.
  - Final shared R4 validation: focused 331 PASS/2 ignored; `cargo clippy -p mc-core --all-targets -- -D warnings` PASS without warnings (OptEntityPhase aggregate, private target, guarded 6G). Exact commands/provenance and all 35 affected file paths are in the residual slice receipt; no duplicate strict gate or native performance claim.
  - R5 residual follow-up: wire publication uses a reusable, mark-time-deduplicated identity queue rather than scanning hash capacity or retaining a candidate vector; idle despawn clocks retain WAL/brain notification without dirtying wire or motion projections. Natural-spawn observer/position scratch retains capacity and skips position assembly when spawning is disabled. Farming folds the immutable sample-seed prefix once per section with identical RNG seeds; portal custody reuses per-pass section verdicts in retained scratch and reads Nether gamerules only on Nether contact. Static r3 attribution puts 80.2% of despawn cost in active-slot admission, not the 100-player position copy; actual wire COW/after-image work remains. Scope, aggregate gates, shared frozen-binary A/B and native-100 acceptance ownership: `.analysis/codex-logs/opt-tick-rest-r5/result.json`.
  - Shared R5 focused validation: 348 PASS/1 ignored across the combined batch and explicit `world::world_tests` supplement (including one repaired overlay-fixture retry); strict `cargo clippy -p mc-core --all-targets -- -D warnings` PASS without warnings. Initial new-fixture compilation and no-op terrain-write assumptions were corrected, with only failed gates/cases retried. Exact logs and source fingerprints are retained in the R5 receipt; frozen A/B is shared with OptEntityPhase, not a native-100 acceptance claim.
  - R6 publication/journal follow-up: journal work has two queued slots plus the executing item; full queues block without dropping accepted work, while disconnect still halts. ADR 0013 distinguishes admission from durable ACK. Initial combined gates passed 408/1 ignored, but frozen R5/R6 BABA found a consistent views regression (peaceful 2.120→2.885 ms; horde 2.221→2.698 ms), so the page batching/staging, projection helper, chunk mutation rewrite and their obsolete tests were reverted. R5 sequential publication remains, with only the measured-positive bulk dedup-control reset retained: same-fixture micro A/B improved 6.6–12.1% versus per-ID removal across six retained/unretained cases. Journal backpressure and serial block-phase comment fix stay. Original comparison: `.analysis/codex-logs/opt-entityphase/result-r6.json`; corrective evidence: `.analysis/codex-logs/opt-tick-rest-r6b/`. No native savings claim; native views timing/profile acceptance remains Main-owned.
  - R7 per-ID wire follow-up: the existing deduplicated candidate queue carries the producer's already-known dense slot. Publication validates that slot against the current identity and falls back to the authoritative ID index after compaction, removal/rebirth, or transfer. Stable candidates avoid an ID-index probe without shifting it to another phase. Wire chunk coordinates reuse the existing exact saturating integer-floor helper, avoiding four out-of-line libm calls per actual replacement; NaN, infinity, negative fractions, signed zero, and saturation retain their old membership. Sequential projection, page COW, snapshot ownership, and WAL policy are unchanged. Farming investigation produced no defensible measured in-scope candidate and no edits. Receipts: `.analysis/codex-logs/opt-tick-rest-r7/`; native timing remains Main-owned.
  - Shared R7 handoff: union focused 136 PASS/0 ignored, then the affected raider lookup regression 1 PASS after obsolete helper removal; strict mc-core all-targets Clippy, scoped format, and release build PASS. Initial missing physics imports and unused legacy wrapper were repaired; an incorrect exact test namespace ran zero tests and was replaced by the exercised one-test receipt. Source digest `965a5d5b…` matches final gates, pre-build and post-build manifests; immutable `core-bench-r7` SHA256 `ba1040b9…`. Corrected attribution uses report sample-RIP, not the first printed DWARF frame: views self is 0.14%, so the hint does not support a multi-ms saving claim. Full evidence: `.analysis/codex-logs/opt-tick-rest-r7/result.json`; no extra core-bench A/B, native acceptance remains Main-owned.
  - R8 lifecycle hint producer: hint refresh was already O(dirty), but healthy adult chickens took two motion-dirty borrows per tick and entered the deduplicated queue for a deterministic egg countdown. Typed harvest-clock writes now bypass hint invalidation as well as the already-lazy journal/wire boundary; absent embedded and other lifecycle clocks take no mutable custody. Egg initialization/reset, growth and appearance events remain marked in the same batch; ordinary breeding/love/child/tadpole clocks remain journal-unmarked. Mixed-NoAI countdown and same-tick growth/egg regressions preserve the lifecycle fences. Main approved leaving required snapshot page COW and real grass/farmland random ticks unchanged; the native fixture contains wheat, not an empty farm workload. Static caller evidence and independent review: `.analysis/codex-logs/opt-tick-rest-r8/`. The targeted ~4 ms native hint-refresh budget is not a measured saving.
  - Shared R8 handoff: one union gate closes 146 unique focused PASS/5 existing ignored, including the <=208-byte worker snapshot bound; scoped formatting, strict mc-core all-targets Clippy and release core-bench build PASS. The initial chicken fixture needed activation after blocking chunk loads. The old age fixture transmuted a cow into an unregistered player; normal Join plus an explicit frozen-world interval now preserves the original +1/global-player and frozen-mob assertions. No production admission/WAL changes were needed for those repairs. Peer changes reuse the snapshot blueprint, narrow rare worker reads to immutable slot custody until Rayon joins, and outline crossed-chunk position bookkeeping. Final corrected gate/pre-build/post-build source digest `df31ac3e…`; sealed `core-bench-r8` SHA256 `2dacfab0…`. Exact failed-first/repair and passing receipts: `.analysis/codex-logs/opt-tick-rest-r8/result.json`; native acceptance remains Main-owned, with no extra micro/native run in this gate.
- Native-100 ≥10 TPS reached 2026-10-17 (uncommitted): r9 single-lookup contact revision proof; r10 page-batched parallel `EntityViews::publish`, parallel `ReadRow` prep (`PrepView` + serial rare-row fix-up), consolidated `apply_motion_result`, villager targets from index, parallel read-only contact/portal classifiers with ordered serial apply. fp native-100 steady 8.98 (r7) → 11.42 TPS (r10); release 11.91/11.56/12.72 steady/burst/recovery, p50 83 ms; gameplay oracle PASS. L2 PASS `.analysis/validation/20261017T175200-correctness-9zikwteb`; independent review pass. 20 TPS at native-100 still open (p50 83 ms).
- Night chain 2026-10-18 (uncommitted, report `.analysis/codex-logs/night-20261018/REPORT.md`): checkpoint redb commit + generated saves moved to a native writer thread (fail-closed on panic/append error, `checkpoint committed` timing line), journal queue 2→128 (bounded), parallel checkpoint entity capture, restored villagers seed journal baselines (no first-tick 500k dump), edge delivers Disconnect reason on core halt, harness `--prepare-concurrency`, piston fixture residency + population fixture z-offset; ADR 0013 updated. Release r12 (solaris-v2 c0f5a0fe): MIX-01 PASS, mixed-survival-long PASS, durability-20k PASS, population-500k (500k villagers + 500 bots) correctness PASS at ~3 TPS / 11.7 GB RSS; native-500/1000 at 100k mobs remain capacity failures (~5 TPS). Small-core frontier: 2 cores hold 20 TPS at 100 bots + 10k mobs (`opt-100/frontier-r10/summary.txt`). C2 kingdoms deferred (`.analysis/committee/decision-012-c2-kingdoms.md`).
- Entity layout + v0.0.8 gameplay 2026-10-18 (uncommitted). Decision 013 (`.analysis/committee/decision-013-entity-layout.md`):
  - Rare components moved to `Option<Box>` (ColdRow 3472→640 B). Six `slots_where` scans and the pickup scan now use sorted lanes. Dense single-owner `Body` (72 B) + horizontal_impulse columns (ColdRow 552 B).
  - fp native-100 steady 14.52 → 15.87 TPS, entity 29.6 → 24.8 ms, RSS −415 MiB. THP (`glibc.malloc.hugetlb=1`) showed no gain and is rejected. Report `.analysis/codex-logs/opt-100/e13/REPORT.md`.
  - r14 fixes: End pillars/gateways, `mth::atan2` guard.
  - Gameplay: player-built iron/snow golems (`golems.rs`; PlayerCreated never targets players), elytra (`tick/elytra.rs`), beacon (`beacon.rs`, `SetBeacon`).
  - PlayerCommand action enum cut over to 26.1.2's 7 actions: START_FALL_FLYING was decoded as ordinal 8.
  - Graphical QA PASS for golems, elytra, beacon and the exact towns guest denied-placement (8/8/8 + rejoin). This clears the 10-15 blocked scenario. Evidence `.analysis/codex-logs/qa-v008/result.json`.
  - L2 PASS `.analysis/validation/20261018T084012-correctness-b5ic4vtv`; harness-check and installer PASS.
  - Release notes `docs/releases/v0.0.8.md`.
  - P2: sibling plugin prebuilts don't start on Main (`basic-economy` capability, `solaris-audit` ABI); CI sibling pins are stale.
  - Next: build --release/java/core-client gates → `.analysis/releases/public-v0.0.8/receipt.yaml` → owner authorization for commit/tag/push.
No commits.

## Core v2 scale balance 2026-10-10 +0700
Route: ADR `docs/decisions/0013-core-model-v2.md` (Threads, Mob blueprints). Owner goal: big worlds without regenerating chunks, mob AI cadence that keeps the world alive, load spread evenly across ticks.
- Edge world credit: `ViewerState.world_credit` = free `world_tx` slots; the plan emits at most that many best-effort frames, so nothing planned is dropped (undo marks removed). Fanout bench 100k/100 viewers: plan 148 -> 17 ms, drops 980k -> 0 (`.analysis/codex-logs/v2-stage1/fanout-credit-bench.md`).
- Chunks: loader workers = cores/2 (TPS 8.4 -> 13.6), `RecentChunks` LRU 2048 returns evicted clean chunks without a load, generated chunks saved in batches of 64 through the journal thread into empty region slots only (`RegionWrite::fill_absent`); worldgen ore halo uses the shared surface sampler, 104 -> 20 ms/chunk, 64 chunks byte-identical (`worldgen-cost.md`). `mark_block_entity_dirty` bumps the chunk revision.
- Mob AI cadence: decisions every tick within 2 chunks of a player, every 2 ticks within 4, every 4 beyond; phase by entity id. Physics/collision every tick: between decisions a mob keeps its last horizontal velocity (resting mobs: see next line; Astra review: thinning physics with a 4x dt step was rejected, a 4x step is not four steps). Test `tick_tests::mob_ai_cadence_follows_player_distance_at_same_speed` (fails on the frozen variant). core-bench 100k cows tick p50: 25.1 none / 14.0 thinned physics / 14.7 as built (`core-bench-spread{,-lod,-lod-phys2}.log`). 100 bots/100k cows, single runs, CPU-bound at 6.0 of 6 quota cores: none TPS 12.8 p50 87 ms (`T070502`), thinned 15.5/48 ms (`T072032`), as built 14.6/71 ms, p95 88 ms (`v2-100k-100bots-20261010T085833`; fanout 1.92, chunk-load 1.72, tick 1.33 cores), 99/100 bots ready (one bot anchor in ocean, bot-side). Gates as built: Java `entities` `20261010T085710`, Java+native death/respawn `…085734`/`…085751`, native `survival` `…085800` all PASS.
- Resting mobs: skipped between decisions only while every chunk under the footprint keeps the revision the previous entity phase read (`TickState.entity_phase_revision`, `ChunkWorld::revision_now`/`chunk_revision`); `/tp` clears ground. Test `tick_tests::resting_far_mob_falls_when_support_is_removed`.
- Profile-driven hot paths (perf via `pkexec sysctl kernel.perf_event_paranoid=1`; driver `.analysis/codex-logs/v2-vs-old/drive_v2_bots.py` env SERVER_CPUS/PERF_OUT/BIN_DIR, records cgroup `cpu.stat`). (1) Unpinned server: CFS throttled 856/929 periods (23 threads on 12 visible CPUs vs 6-core quota); pinned 0 throttled. (2) Worldgen: ore cells above the cached cave layers ran a full uncached density eval per block (`OreColumnCache::get_or_cave` fallback); now through the chunk's halo `SurfaceHeightSampler` + per-cell memo: mountains 624 -> 29 ms/chunk, identical block hash (`worldgen-cost.md`). (3) SipHash in tick/fanout maps -> `rustc_hash` Fx in `world.rs`/`tick.rs`/`plan.rs`; `block_at` skips the empty overlay. (4) `play_writer` joins queued frames into one write (<= 64 KiB): tokio threads 1.06 -> 0.24 cores. 100 bots/100k cows release: before TPS 16.0 tick p50/p95 62.7/78.5 ms (pinned `v2-100k-100bots-pin6-20261010T091027`); after TPS 20.0 p50/p95 22.7/32.4 pinned (`…-fx-20261010T10*`), 16.3/18.1 unpinned (`…-fx-unpinned-*`, throttled 3/700), server 3.2 of 6 cores. No unsafe needed.
- Gates on the final tree: L2 PASS `.analysis/validation/20261010T101445-correctness-n6y1i5k7`; Java `entities` v2 `…101333`, native `survival` v2 `…101438`; death/respawn Java 4/4 + native 6/6 after probe fixes (`native_lifecycle.py` waits for play after Respawn and for the death screen; both were probe races). Reviews: cadence pass, respawn pass after GameEvent 13, hot paths pass. `harness-check` fails `LoadMatrixCliTests.test_full_off_core_lifecycle_keeps_long_window_and_single_cpu` also with HEAD versions of the touched harness files (not from this work, not investigated).
- Same-dimension respawn (owner: vanilla clients must work): vanilla 26.1.2 `ClientPacketListener.handleRespawn` keeps its `ClientLevel` unless the dimension changes, so the edge (`plan.rs` `forget_all`/`forget_everything`) now does as the vanilla server: on the own `Respawned`, the next world plans first send RemoveEntities (all held) + ForgetLevelChunk (every held chunk) on the world queue, after frames already queued and within credit, then resend the whole view including self data. The Respawn burst now also sends GameEvent 13 (`EVENT_START_WAITING_FOR_CHUNKS`); without it the Java client stays on LevelLoadingScreen forever (review finding, verified with javap `handleGameEvent` -> `loadingPacketsReceived`). Tests `plan_tests::respawn_forgets_everything_held_before_resending`, `respawn_forget_pass_waits_for_credit`, `respawned_plans_respawn_then_position_sync`. The `native-lifecycle` probe now also runs with `--client java` (guard removed in `client_probes.py`). Death/respawn on v2: Java PASS `.analysis/validation/20261010T081704-native-lifecycle-p595tnzv` (death screen, respawn, screen closed, same dimension), native PASS `…20261010T081449-native-lifecycle-09a1fd9i`; Java `entities` v2 PASS `…20261010T081500-entities-vz18y9bi`. Not proven by a client gate: absence of death-site ghosts (unit tests only). Known, not fixed: a queued pre-death self SetEntityData (health 0) overtaken by Respawn can reopen the death screen.
- Generated saves are bounded: `generated_busy` allows one `SaveGenerated` queued or running; the world holds at most 1024 more (`GENERATED_PENDING_MAX`), beyond that chunks are regenerated when next wanted.
No commits.

## Core model v2 checkpoint 2026-10-09 +0700
Route: ADR `docs/decisions/0013-core-model-v2.md`; live run cursor `docs/NIGHT_RUN_2026-10-08.md`.
New core `crates/mc-core` (tick/merge/journal/checkpoint, Main's pure modules `inventory/`, `survival.rs`, `combat.rs`, `commands.rs`, `player.rs`, `block_drops.rs`) behind new edge `crates/mc-edge` (`solaris-v2` bin, harness `--server-variant v2`). Old core untouched as v1 default.
Real client on v2 (Xvfb): `core-client` PASS. Java `survival` was flaky (one pass in 14 runs): the client loaded fine (jcmd: `LevelLoadTracker$ClientLevelReady`, no `LevelLoadingScreen`), but v2 sent no traffic at all to an idle player, and the harness agent counts applied packets, so `wait_for_state_change` starved. Fix: `mc-edge` `plan_viewer` sends `SetTime` every 20 ticks (as vanilla does; interval-based because the pump coalesces outputs), and the join burst's game_time is now `snapshot.tick`. Test `plan_tests::idle_viewer_gets_world_clock_every_20_ticks`; diagnosis `.analysis/codex-logs/java-stall-probe/`. After the fix: Java survival 3/3 PASS (`.analysis/validation/20261009T034151-survival-6rbrb3b7`, `…034218-survival-nyjzv2g8`, `…034241-survival-pucmm6ya`), native survival PASS `…034230-survival-jgljwjuj`, native death→respawn `native-lifecycle` PASS `…034245-native-lifecycle-_a1ltnv4`. Native client env: `set -a; . .analysis/codex-logs/core-v2-review-fixes/client2.env; set +a`.
Durability: full checkpoint in vanilla format (player `.dat` storage slots 0–35 + `equipment` compound via `mc_data::player_nbt`, `entities/r.x.z.mca` IntArray `Position`, DataVersion 4790, stale regions removed, chunk `block_entities`), base-first replay fold, write-ahead ack gate, prune to previous manifest, journal poison on append failure, shutdown joins tick→journal, lossless cursor/grid (`solaris:Carried`/`solaris:CraftingGrid`), offline players on Leave, contested placement charges only the winner (sync after debit), container break drops contents, reliable outbox never drops (`PlayerChange::Disconnect`), chat via reliable outbox. Independent review (17 P1 + 3 P2: `.analysis/codex-logs/old-vs-v2/review-findings.md`) all fixed with regression tests incl. a real SIGKILL child-process crash test.
Perf (release, `-x`): the core-only bench put v2 at 38.1/41.2/42.4 ms against the old 45.7/56.0/62.0 ms with 50 clients. **Server vs server** (2026-10-09): `solaris-v2` with 100k cows and 50 `player-scale-bots` at VD 6 runs a core tick of p50/p95/p99/max 31.7/41.0/47.7/106.0 ms against the old 45.7/56.0/62.0/1437.6 ms, TPS 19.87. The pump fanout of 18.5/22.3/30.3 ms runs in parallel off the tick thread. v2 clients each see ≈10.8k cows, against ≈4k for the old clients. Single run per side; `.analysis/codex-logs/old-vs-v2/RESULT.md`, receipt `.analysis/codex-logs/v2-vs-old/v2-100k-50bots-20261009T065035/`.
Bench timing bound to its tick: `boundary::TickTimes` (+`total_us`) rides in `TickOutput.times`, set before the broadcast; `TickStats`/`Core::tick_stats()` removed; `core-bench` reads `output.times`. Halt fix: `CoreInner.tick_tx` is a `WeakSender`, so a journal halt or shutdown closes every subscriber (ADR 0013 Durability). Previously an L2 hung 54 min in `core_tests::overflow_seals…`: the disk was full, the journal died, and nobody saw `Closed`. Also fixed: a shared cap-2 temp dir in `core_tests`, a flaky `container_break…` test (`settle()` = 3 fresh ticks), and a scheduling-dependent tick bound in the poison test. Known gap (prio 4): the edge pump stops on `Closed` without a final `route_outbox`, so halt-sealed `Disconnect`s are not delivered.
L2 PASS: `.analysis/validation/20261009T051713-correctness-656eqtrb` (5978/0). It needed `SOLARIS_HARNESS_MEMORY_MAX=8G`: twice the default 4G scope was OOM-killed, once while rebuilding after the disk cleanup and once in the `mc_net` unit tests. Disk was 100% full: freed `target/debug/incremental` and `deps` artifacts older than 12 h. Reviews: time sync pass; core timing/halt pass after the P2 fix. Maturity: draft. No commits.
Slice B (fixes the bench needed): the dashboard `/stats` `tick` now reports the core tick cost from `TickOutput.times`, with a new `publish` window. `solaris-v2` previously served a second `DashboardState` that nothing updated, so `/stats` was always zero. Entity/chunk counters were `connections.len()`, and bytes were added cumulatively every tick; both now come from the snapshot or the counter. `--bench-seed-cows N` was added. `/tp` now sends `PositionCorrected` to the teleported player; before, the client never moved and its next move dragged the server back. Tests: `teleport_command_syncs_the_teleported_client`, `tick_percentiles_report_core_cost_and_publish_time`. Review pass. L2 PASS `.analysis/validation/20261009T065608-correctness-hroiwbzv` (5979/0, `SOLARIS_HARNESS_MEMORY_MAX=8G`). `/tp` is proven by 50 protocol bots (readiness requires a SyncPos at the target) and by the unit test. A graphical client has not exercised `/tp` on v2: no v2-capable harness scenario sends `/tp`, and `regression` for m94-03b ignores `--server-variant` and starts v1. Known limits: no ConfirmTeleport gating (a stale move within 10 blocks can briefly undo a tp, then it converges), teleport ids are reused, and `player-scale` harness parsing expects the v1 `/stats` schema.
Slice C (v2 real-client entities/inventory, 2026-10-09): v2 had no pig, no feeding (`InteractEntity` was a no-op), no mob death loot, and `entities_appearance_changed` was never filled (clients never saw mob health change). Added: `minecraft:pig` blueprint; `MobBlueprint.breed_food`/`loot_table`; `IntentKind::FeedAnimal` (inventory claim takes 1, 0 in creative); love state `ColdRow.love` (600 ticks, not persisted) — breeding needs both partners in love and the same type; merge `kind_valid` re-checks feed/breed, so a second feed or breed in the same tick pays nothing; spawned adults start at cooldown 0, newborns get the blueprint cooldown; summon uses blueprint max_health; `mob_loot.rs` rolls the embedded vanilla entity loot (no attacker: looting/player-kill rare drops never fire); `TickState.appearance_changed` (mob health, partial item stacks). Test `feeding_gates_breeding_and_takes_one_food`. Real client v2: entities PASS native `.analysis/validation/20261009T093827-entities-zr40i_mw` + java `…093902-entities-09wq76h3`; inventory native PASS `…093927-inventory-2tvb4ebs` (the earlier `cursor_roundtrip` failure did not reproduce in 6 runs; server traces showed one atomic image per click — suspected client observation race under host load, not root-caused). Review: 2 P2 (double feed, pig×cow breeding) fixed. Java inventory and regression ignore `--server-variant` (they run v1) → no v2 evidence from them.
L2 PASS `.analysis/validation/20261009T093940-correctness-ch4awyvb` (5980/0, `SOLARIS_HARNESS_MEMORY_MAX=8G`). Disk cleanup: stale `target/debug/incremental` and `deps` older than 4 h removed, 12G → ~90G free. No commits.
Slice D (v2 Java regression + client gates, 2026-10-09): Slice C's claim "java inventory/regression ignore `--server-variant`" was half wrong — `regression` already ran `solaris-v2`; java `inventory`/`core-client` used the Loader fixture path (v1). Fixed `tools/harness/profiles.py`: v2 routes java `inventory`/`core-client` to the shared probes. `regression` m94-02b on v2 at view 8 stuck the Java client on `LevelLoadingScreen` (no packet for 5 s). Two causes, both measured with server traces: (1) `play::send_join_burst` encoded the whole view square inline (~11 s debug for 289 chunks) on the connection task — removed, the pump owns chunk streaming; (2) the pump never yielded while catching up (`rx.recv()` Ready), so the connection task woken into its worker's unstealable LIFO slot sat behind it with 512 routed chunks — `tokio::task::yield_now()` after each publish. Plus `plan::CHUNKS_PER_TICK = 16` nearest-first pacing (one unpaced publish took 11 s and stalled every viewer). Also: `World::ensure` loads/generates missing chunks over the worker pool (serial gen hit the 10 s join wait); respawn now restores published `appearance.health` (death zeroed it and the resent self SetEntityData re-killed the client; test asserts it). m94-02b PASS `.analysis/validation/20261009T103504-regression-c_zmoazm` (fresh world, `SOLARIS_REAL_CLIENT_FRESH_WORLD=1`; never reuse repo `world/` with v2). After the edge change all v2 real-client gates PASS: entities native/java `…103718-entities-7ss_g10u`/`…103730-entities-fsmug_1t`, inventory native/java `…103751-inventory-xxe53nal`/`…103800-inventory-drzs22w1`, core-client java/native `…103818-core-client-_trblm05`/`…103835-core-client-1vork_em`, survival java `…103841-survival-da1uol9a`. BLOCKED on terrain: m94-03b two-client chest (`…103555-regression-jx3tznk6`, tp passed, "no loaded dry chest placement target within survival reach" — v2 ignores the example.toml world profile/spawn finder and spawns at 8,80,8).
L2 PASS after Slice D `.analysis/validation/20261009T103904-correctness-a9y2m58v` (5980/0). No commits.
Slice E (lost spawns on a full outbox, 2026-10-09): when the pump planned an `AddEntity`/chunk/forget/remove, it marked the viewer cache optimistically. If the BestEffort `world_tx` dropped the packet, the cache stayed wrong forever: a mob the server considered "known" never appeared. Fix: `plan::PlannedPacket.view: Option<ViewMark>` (`Chunk`/`Forgot`/`Spawned`/`Removed`) is carried through `fanout::AddressedPacket.view`; on `try_send` Err, `server::publish_and_route` calls `plan::undo_view_mark`, so the next plan resends. Test `plan_tests::dropped_spawn_and_chunk_are_resent`. Real-client A/B, 600 cows, tp away then back (`.analysis/codex-logs/v2-vs-old/dense_rejoin_probe.py`): fix [200,200,200] (`.analysis/validation/20261009T114955-entities-llfqfy4b`), undo disabled [200,48,0] FAIL (`…20261009T115438-entities-5blkh58e`). omp crash RCA 10-08: `.analysis/codex-logs/omp-crash-rca-20261008/REPORT.md` (external dispose, not OOM; agent.db integrity ok).
Slice F (v2 worldgen + off-tick chunks, 2026-10-09): v2 built a bare `TerrainGenerator` and spawned the player at fixed 8,80,8. It now loads the vanilla router (`load_vanilla_worldgen` from the edge's `vanilla_data_dir`) and picks spawn through the shared `TerrainGenerator::locate_safe_spawn` plus a block-level dry/clear check; the spawn persists in `CHECKPOINT-MANIFEST.json` `world_spawn`. With vanilla density the first join timed out: `World::ensure` generated 289 chunks synchronously inside the tick, which violated ADR 0013. New `mc-core` `chunk_loader` is a fixed worker pool that each tick re-queues missing chunks nearest-first and drops stale ones. `ensure` never blocks; results join at the next input step; `ensure_blocking` exists for tests only. `desired_chunks` now returns pos→priority. The real-client scenario `m94-03b` (two clients sharing a chest) hard-coded a teleport of the primary to (2.5,81,0.5), a pre-spawn-finder layout; there is no receipt of it passing on v1 either. `../solaris-loader` `M94InventoryCraftingScenario.java` now places the chest at the shared world spawn (28 lines deleted). Gates: m94-03b PASS `.analysis/validation/20261009T125115-regression-7n8hm4eq` (chest at 127,71,319, secondary opened it), Java survival PASS `…124904-survival-46flkclc`, mc-core 121/0, mc-edge 87/0.
Review (independent, 2 P1 + 1 P2, all fixed with regression tests):
- (P1) A position stayed `running` only until it was sent, so a request between send and drain queued a second load. That load's stale result could restore pre-edit terrain after a save and eviction. Now `running` clears only when the tick consumes the result (`ChunkLoader::consumed`).
- (P1) `flush_dirty` ignored overlay writes for chunks that were not resident (journal replay, or a chunk still loading), and the checkpoint then superseded their journal records. Pre-existing, widened by async loading. Now it materializes those chunks and folds the writes first. Test `world_tests::checkpoint_persists_writes_for_nonresident_chunks`.
- (P2) Undoing a dropped `RemoveEntities` restored only `known_entities`, so the entity stayed frozen on the client. `ViewMark::Removed` now carries the dropped movement baselines. Test `plan_tests::dropped_removal_keeps_moving_the_entity`.

- (advisor, blocker) The sequence: a worker's load is in flight, `flush_dirty` materializes and saves the same chunk, the chunk is evicted and wanted again, and the old result lands, reverting the saved edit both in memory and on the next flush (repro `.analysis/codex-logs/watchdog-late-load/`). Fix: `ChunkWorld::materialize` calls `ChunkLoader::withdraw`, and a load a worker already took is marked `superseded`; `accept` discards that result. Barrier test `world_tests::late_load_never_reverts_a_checkpointed_edit` fails with the marker disabled (`.analysis/codex-logs/v2-async-chunks/late-load-ab.log`).

mc-core 123/0, mc-edge 88/0. Also fixed: harness `native_playthrough.py` referenced an undefined `options_shot`; the client env gained the windowed asset roots.
L2 PASS on the final E+F tree: `.analysis/validation/20261009T131917-correctness-lx4py1kb` (5985/0). No commits.
Next: 100-client v2 run (mandate #4); HN numbers (owner decides).

## Settlements live closeout 2026-10-07 +0700
Candidate: core worktree unchanged since L2 PASS `.analysis/validation/20261006T145311-*` (HEAD `1fde9ec0`, `git diff HEAD` sha256 `47db2db3…28b72d9`, untracked sha256 `95b26b46…c981f`; `final8-matrix/tree-fingerprint.txt`). Plugin package `/tmp/cp-final6b-pkg`: settlements `plugin.wasm` `2a0f3b2d…eadeea79`, source `caravan_runtime.rs` `03112bfe…` (receipt `content-receipt.json`).
Last defect closed: `caravan-status` polls (every tick) won the pending race against the Wake timer and answered status while a durable Cancel intent stayed unresolved (`car-effects`: native Cancelled/epoch 2, plugin Arrival/1). Fix is only `Action::Status` + `Intent::Cancel{epoch}` + native `Cancelled` with `executor_epoch > epoch` (AliveLoaded) completes the Cancel from the same query; every other pending intent stays read-only. Tests: `caravan_runtime_tests::status_poll_with_pending_cancelled_ack_completes_cancel` and `status_poll_with_unconfirmed_intent_stays_read_only` (fails on the broad variant); plugin crate 334/0.
Live receipts (all on v6b, `.analysis/codex-logs/cp-live-20261004/final8-matrix/`): `car-effects` 3/3 PASS (`effects-a`, `effects-b`, matrix run; 853-881s each); 13-gate matrix 13/13 exit=0 (commerce 1, production 5, caravan 7); headless war `settlement_war_chain` PASS 3943s; Xvfb real-client `settlement-war-breach` PASS 3735s (`.analysis/validation/20261007T013105-settlement-war-breach-16z2x88j`). One earlier Xvfb attempt (`20261006T234004-…-rpx1tf6y`) failed in 101s with `Building recognition refused: stale-revision` while the matrix ran concurrently on the same host; the serial rerun on an idle host passed. Not root-caused: survey→recognize race under CPU load, treat the graphical war as host-idle evidence.
Limits unchanged: one account owns both settlements in the war scenario; the real 72000-tick grace is wall clock; draft maturity. No commits.
Kill-test block 1, Solaris scale curve DONE (`.analysis/codex-logs/scale-curve-20261006/RESULT.md`, internal, bots share the cpuset, single run per point): 100k entities/25 regions/50 clients tick p50 108.4/67.7/49.3/42.4 ms on 1/2/4/6 cores, 41.5 ms on 12 SMT uncapped (quota 1200%; the default mem-run CPUQuota=600% does not explain the plateau); p95 118.9/78.6/58.2/50.4/48.9 ms vs 50 ms budget. Speedup 2.56x at 6 cores. Stage p50: entity_physics 55.5→19.1 ms and entity_goals 33.2→12.9 ms (2.6-2.9x), flat stages ~9 ms; lane CPU max/mean 1.04/1.13/1.20 equals the 25-equal-region granularity bound. `session_lock_wait/hold` are cumulative over threads, not a serial floor. Plateau cause not isolated. Owner decision 2026-10-07: do NOT compare with Folia/Pumpkin yet (fixtures kept in `.analysis/bench/competitors/`; existing bot is Solaris-sequence-specific).

## Night-run closeout 2026-10-04 ~evening +0700
HEAD `1fde9ec01f168f699c4bac7bada7230f93ba924b` (stack `fd6a8260`/`c3b12b07`/`8f718f42`/`1fde9ec0` on `1be5f4db`; worktree carries large unrelated diffs + untracked `crates/mc-test-harness/src/bin/player_scale_bots.rs`). L2 rust gates PASS — `.analysis/validation/20261004T100704-correctness-w275rl94/result.json` (profile `correctness`, scope `rust`, 10:07:04→10:24:54Z, exits `[0,0,0,0]`). Matrix authoritative 4/12 PASS (spread-passive, fluids, village, farms) — `.analysis/codex-logs/load-matrix-20261004/verdict-20261004.yaml` (run `matrix-20261004T105818-6634bdd2`, harness `.analysis/validation/20261004T105818-load-matrix-dvixa92r`); FAIL dense-hostile TPS 18.903, explore activity Δ0/16 (TPS 20.001 healthy), built-base TPS 12.081 (block_tick p95 80836us), combat TPS 4.74 (tick p95 309.21ms), projectiles activity Δ0/1 (TPS 20.0 healthy), plugin-heavy infra (fixture-bootstrap exit 1), checkpoint-restart TPS 13.738 (persistence proved), long-session TPS 15.889. Profiled run `matrix-20261004T104033-569840c4` INVALID (perf_event_paranoid=4, profiler exit 255).
Hotspot slices landed (worktree): BlockTick halo bound + partner corner fix (~2.8x focused repro 532→187ms/960 plans; 46 tests) — `.analysis/codex-logs/BlockTick-20261004/report.md`; CombatGoals batch fix (driver unresolved, ~290ms unattributed) — `.analysis/codex-logs/combat-goals-20261004/report.md`; ActivityFix explore PASS (chunks 3681→6684 Δ3003, TPS 19.9997) — `activityfix-verify-explore/matrix-20261004T130257-17d436e6/01-explore/initial/receipt.json`; projectiles proj2 PASS (4468→6037 Δ1569, TPS 19.9977, `--anchor-chunk-walk` = `1fde9ec0`) — `activityfix-20261004-proj2/matrix-20261004T123223-35804f08/01-projectiles/initial/receipt.json`; proj3 dedup rerun PASS (deduped 343→473 Δ130, TPS 20.0104) — `activityfix-20261004-proj3/matrix-20261004T133855-41212c14/01-projectiles/initial/receipt.json`.
Advisor warns ACCEPTED: (a) firing-proof `.analysis/codex-logs/activityfix-20261004/firing-proof.md` downgraded to gate-PASS — sustained fire unproven (UUID first-sighting ≠ launch moment; fire freezes ~tick848 vs window to 1609; in-window respawns restream, proj3 `respawns_received` 16); (b) settle package acceptance reopened as startup-only — CLI covered only empty-world `--inspect` RC=0 + `--dry-run` RC=1 refusal, `crates/mc-plugin-host/src/check.rs:250-262` stages-but-never-fires timers, no `events_delivered>0` for settlements; redo IN-FLIGHT under FamousCentipede. Package republished (uncommitted, plugin repo): `plugin.wasm` d0cb0216→`9d855c3b…5d310`, client zip →`b9e70ae9…dfdf`, WIT-closure `3a82bac2…` — `../solaris-default-plugins/.analysis/settle-package-20261004/report.md` (§4 ABI smoke startup-only: `--check`/`pregenerate`/fixture-prepare RC=0; full row + `settlement-cargo` 1/2 pending).
Real-client: core-client FAIL (hooked `minecraft_wait_for_block_state`: placed-31 witnessed, state event never arrived; wasm-server-only + precommit-direct PASS; inventory/replay/playable NOT RUN) — `.analysis/codex-logs/realclient-20261004/realclient-gates-20261004.md` (harness `.analysis/validation/20261004T132614-core-client-eucnrnpe/`).
Remeasure (fixed binary `50401d1f…`, build `.analysis/validation/20261004T130608-build-ay46ip8t`): combat TPS 4.58666252972561 still FAIL — `remeasure-20261004/matrix-20261004T130647-c02d48bf/01-combat/initial/receipt.json`; built-base infra `bot 24: peer closed` (server-side mid-play TCP drop, cause unlogged; latest-sample TPS 10.36, not a perf verdict) — `remeasure-20261004/matrix-20261004T131240-13552226/01-built-base/initial/receipt.json`; triage `.analysis/codex-logs/hotspot-closeout-20261004/triage-built-base-combat.md`.
Baseline A (gitignored, never stage): `.analysis/baseline-20261004/` (README, SHA256, SHA256.B: server `50401d1f…`/bots `5300ecf8…`/fixtures `ad4b76ab…`, SHA256.src, `bin/`, `srcA.tar.gz`).
IN-FLIGHT (never green): ShallowTapir hotspot fixes (mc-net connection/save/TNT instrumentation); FamousCentipede CLI chain + live settlements timer proof; CP audit (CP-050…096 proof pending); real-client phase 2. Pending honestly open: combat/built-base/checkpoint-restart/long-session/dense-hostile TPS, plugin-heavy full row, CP-050…096, real-client. Next: release rebuild + targeted remeasure + QA stage (Main's call). No commits.

## Night-run closeout 2026-10-03 ~15:30
Single identities: freeze-r3 snapshot (2968 files), server
`bin/mc-server-final-r3` (`a279a67d…9d223d`), base `4865344e…` + uncommitted
patches. Verified: stack-overflow fix (Box::pin, no recursion) + restore
forward delta; mc-net lib 2178/0, wasm 6+2+3+2+1/0, plugin-host 140/140,
server bin/cli/play 136/0, replay driver 28/0; clippy/code-health/fmt green
on snapshot. Matrix non-plugin: 6 PASS / 5 TPS-or-evidence FAIL (dense,
built-base, combat, checkpoint, long-session); hotspots worldgen/network/
scheduled assigned, none closed by measurement. 50-client rung: TPS 20.0,
p95 18.7ms, 28.5% CPU (only 4 bot-side water timeouts). Settlements: private
r3 package built, `storage` grant missing (config staged, unvalidated).
Native gates on empty-plugin fixture: nav 82/82, Loader 8/8, food 10/10.
CP058/059/068 DECIDED 2026-10-04 (council Astra+GLM, owner may edit): recorded in
docs/settlements/OWNER_DECISIONS.md; OPEN only mounted feed/HP/stamina/collider/seat/impulse native profile. Other blockers: no L2 `correctness` profile (4G OOM), no plugin-heavy row, no r3-pair acceptance. No commits.

## Absorption correction — 2026-10-02

Player additional hearts are authoritative in `SurvivalState::absorption`.
Golden/enchanted apples and every shared Absorption application grant
`max(existing, 4 * (amplifier + 1))`, including hidden/unchanged effect merges.
The shared damage helper spends them after equipment reductions; effect expiry,
hidden-level downgrade and milk cap/clear the amount. Playerdata stores vanilla
`AbsorptionAmount`; owners and trackers receive FLOAT player metadata index 17
(local 26.1.2 `javap`, including the Avatar parent).
Receipts: `.analysis/codex-logs/absorption/` — original golden-apple arrow repro
failed with health 17 instead of 20; final five scoped regressions passed,
including partial save/reload and real loopback late-tracker metadata; strict
Clippy on mc-net/mc-simulation passed; code-health reported 0 fail.


## Текущая работа — Core Architecture Program (CP-ARCH, начата 2026-09-28)

Владелец запросил оценку архитектуры ядра и её фиксацию. Программа CP-REFACTOR-000…100
закрыта (файлы разрезаны, навигация построена, ownership не трогался — §5.4 старого
плана). Аудит семи срезов нашёл одну корневую проблему: граница simulation/application
core существует как соглашение, а не как crate — весь движок в `mc-net`, `SimulationOwner`
зависит обратно на `SessionRegistry`, два писателя player inventory, policy в адаптерах,
composition root в `mc-net::bind_internal`, production зависит от `mc-test-harness`, tick
блокирует tokio worker, shutdown может отменить final save. Решение — ADR 0011: один
новый crate `mc-simulation`, направление зависимостей защищает Cargo, tick на выделенном
потоке. Очередь CP-ARCH-000…070: [CORE_ARCHITECTURE_PLAN.md](CORE_ARCHITECTURE_PLAN.md).

```yaml
route: architecture
plan: docs/CORE_ARCHITECTURE_PLAN.md
decision: docs/decisions/0011-core-crate-topology-and-threading.md
evidence: docs/evidence/core-architecture-audit-2026-09-28.md  # findings A1..C10; raw reports .analysis/codex-logs/cparch-000/
current_checkpoint: parallel tracks (owner 2026-10-01). TICK: main has TickGoals+EntityIdHasher+TickHash+TickPhys; living-world still over budget (worktree A/B 74/172/200 ms p50/p95/p99, receipts .analysis/codex-logs/{tick-goals,tick-hash,tick-phys}/); TickTail fixing the measured tail = startup owner-lane collapse (regional work invisible to adaptive CPU pool, mc-net server/runtime_control.rs). WORLDGEN: W0+W1+W2 integrated (docs/evidence/worldgen-w2.md; C04 deferred to W7, delta inference to W3); W3 in progress; vanilla-parity patch rides with the purge. CORE PURGE + settlements A0–A8 / CP-049…096 in solaris-settle + plugin repo. CLEANUP integrated: clean-core/world/harness/net1/sim/data/test-harness. NATIVE CLIENT: new repo ../solaris-client (spec ~/Downloads/SOLARIS_NATIVE_CLIENT_SPEC_FOR_OPUS.md), C00–C02. Knowledge log: docs/memory/log-18-2026-10-parallel-tracks.md.
last_closed: main L2 correctness PASS on the combined tree (2026-10-01, .analysis/validation/20261001T152611-correctness-igq49dkj). Flaky farmland-trample test fixed (fixture authority mismatch, .analysis/codex-logs/flaky-trample/report.json)
quality: draft
base_trees:  # core HEAD at audit; no push
  solaris: 4865344edb3733eacd5a942cdd7b548f39a80126
  solaris-default-plugins: 9adf5aebceb7595ca75f8e93ad82ef01739d328a
  solaris-loader: bab2329c707d4bff026f22b65162e93b432ab50c
ci_sibling_revisions:  # .github/workflows/ci.yml source pins
  solaris-default-plugins: 9adf5aebceb7595ca75f8e93ad82ef01739d328a
  solaris-loader: bab2329c707d4bff026f22b65162e93b432ab50c
untracked_orphans: ../solaris-default-plugins/sources/solaris-settlements/src/{command_dispatch,legacy_commands,lib_tests,registry,warehouse_policy}.rs are not wired from lib.rs (abandoned CP-040 split); excluded from commits, owner to decide
owner_dirty_preserved: WATCHDOG.yml, tools/harness/{loader,profiles,scenarios}.py (not touched by CP-ARCH-000)
paused: CP-ARCH-040…070 queued behind the owner-ordered tracks above (freeze on CP-049–096 lifted 2026-10-01)
next: integrate TickTail → exclusive living-world-scale + entity-scale; integrate SettlementsCP049 (purge + generic API + vanilla worldgen, 3-way vs main) → L2 + core-client + loader-live + real-client village check (seed 497625, chunk 42,-33); integrate W3. Open owner decisions from W0: cold/root SLOs, generator artifact retention, never-activated inhabitants/loot export policy, accepted import losses. Commit/push not authorized.
```

**CP-ARCH-030 (2026-10-01, source, closed — step 6 accepted by owner).** Receipt `.analysis/codex-logs/cparch-030/receipt.md`
(шаги 1–7, diff_hash там). Один writer: inventory (owner reservation для compound), shield
(`PlayerStateEvent::Shield`), world time/night skip/world rules (`SetWorldRule`; sleep tick перед
time advance), plugin teleport (`CommitScriptPlayerTeleport` + `teleport_generation` fence,
`OutboundCommand::TeleportPlayer`; `TeleportPending` удалён по решению владельца, WIT-кейс оставлен).
Policy → owners: menu (crafting/stonecutter/enchanting/recipes), attack costs, script world-block
planner → `mc-simulation`; fall landings из ECS + passive/dragon/villager/hostile planners +
`EntityClassIndex` → `mc-entity`. Гейты: L2 `correctness` `20260930T144051`; real-client inventory
`20260930T113600`, core-client `20260930T143105`; TCP block_edit/sleep/commands/wasm_* по шагам.
Scale fix 2026-10-01 (`.analysis/codex-logs/scale-fix/receipt.md`): entity lanes follow the CPU pool
(physical-core ceiling); temptation reads only animals near food holders (was a full 40k snapshot copy per
tick); connection serves outbound while one movement commit is in flight (merged samples). entity-scale
PASS 60/60 (p95 140 / p99 192 ms); living-world-scale no disconnects, fails tick budget (p95 191 / p99 199 ms
vs 50/60). Test-spy cleanup (24 files, −1223 lines). L2 `20261001T055505`, core-client `20261001T050354`.
Review `CparchReview`: F1 stale movement vs teleport → generation fence; F2 shield → sequenced
owner clear; F3 water flags после commit → принято (выбранная владельцем семантика).

**CP-ARCH-020 (2026-09-29, source, closed).** Receipt `.analysis/codex-logs/cparch-020/receipt.md`
(шаги 1–9, отклонения от pure move перечислены там). Новый crate `mc-simulation` без `mc-net`/
`mc-protocol` (`cargo tree` = 0): lock policy/metrics, kernels, `persistence` (world/inventory/
regional journals, player/world/entity codecs, dirty-flush), `services::{storage,residents,
settlement}` (`PluginStorage` + ledgers). В `mc-net` остались адаптеры: `PlayerPose`/
`InteractionState`, player runtime adapter, `resource_profile`, storage actor, `InventoryRuntime`,
`*_runtime.rs`. C3: `StorageSlot::durable` (spawn_blocking) для всех durable append actor'а и
стартового recovery; 6 current_thread драйверов: before 6 FAILED → after 6 passed. Тестовые seams —
feature `test-hooks` только из dev-deps `mc-net`. Сборка focused-теста: kernel в `mc-simulation`
~7 s (было 45–58 s через `mc-net`); `mc-net` touch 45 s (HEAD 49 s). Гейты: L2 `correctness`
`20260929T170532` (с `SOLARIS_HARNESS_MEMORY_MAX=7G`: дефолтный scope 4G ниже пика rustc
`mc-net` ≈4.5 GB — OOM-kill без receipt), `code-health` 42 правила + 5 bypass-проверок
перенацелены на sim, `architecture --check` current, сценарии warehouse-bind-manual/-refusals/
-reservation-restart, warehouse-transfer-recovery, inventory-storage-recovery,
`settlement-cargo` `20260929T175754` — passed. Sibling `solaris-default-plugins`
integration-tests: импорты `mc_server::wire_client::Client`, `mc_simulation::tick_policy::
RandomTickPolicy`, dep `mc-test-harness` удалён (локально, не закоммичено). Independent review
`CpArch020Review`: pass, 0 findings. Real-client не требовался (client-visible surface не менялась).

**CP-ARCH-010 (2026-09-28, source, closed).** Receipt `.analysis/codex-logs/cparch-010/receipt.md`.
B3: `mc-test-harness::{client, registry_capture, VanillaServerProcess, parse_java_major_version}`
переехали в `mc-server::{wire_client, vanilla::{process, registry_capture}}` (git mv, verbatim);
`mc-server` больше не зависит от `mc-test-harness`, harness зависит от `mc-server`; 34 test/bin
callers переписаны, re-export'ов нет. B6: `MoveEntityRot` в `mc-protocol` + wire-layout тест.
B7: projectile `EntityId` → `mc_entity::EntityId`, конверсии удалены; `BlockStateId`/`BlockPosition`
оставлены с обоснованием (serde persisted state, `mc-world` без serde). C5: supervisor
deadline и ticker drain больше не отменяют final save / simulation tail — классифицируют как
`failed` после завершения. C6: `serve` join'ит storage actor после periodic-save drain и до save
barrier. C7: dashboard — JoinSet + лимит 32. Review `changes` → 2 findings исправлены: 9 JSON
manifests/README перешли на идентичность `mc_server::wire_client::Client`; слабый abort-тест
dashboard удалён. Advisory после review: новое ребро harness→mc-server втянуло
`#[global_allocator]` (stats_alloc) из lib в harness-бинарники (`load_scenarios`) —
allocator перенесён в `main.rs` (binary), lib только читает счётчики; `nm` подтверждает
(0 `StatsAlloc` в `load_scenarios`, есть в `mc-server`). Гейты: `fmt` `20260928T231128`,
`code-health` `20260928T231044`, strict `clippy` workspace `20260928T204922` + targeted
`mc-server`/`mc-test-harness` после allocator fix clean, `harness-check` `20260928T210025`,
`fixture-check` `20260928T210025`, `architecture --check` ok; focused tests
(server/protocol/harness lib+bins 532, mc-net `server::tests` 108, harness TCP `commands` 6,
`real_client_manifest` 49, projectile 58+31, dashboard 15, shutdown 2, profile 3). L2
`correctness` и real-client — не запускались (client-visible surface не менялась; L2 — на
закрытие native-волны CP-ARCH-020 по §6 плана).

**CP-ARCH-000 (2026-09-28, docs-only).** Написаны: audit (27 findings, каждый с
инвариантом и `file:line`; blocking: A1 два writer'а inventory, A2 back-edge
`simulation.rs:36-65,4561-4619`, B1 движок в `mc-net`, B2 два composition root, C1
tick в tokio-задаче `server.rs:1758-1762` + blocking `handle.rs:2375-2385`), ADR 0011
(топология, threading, четыре durable stream с acknowledgement), план CP-ARCH-000…070 с
картой переездов; `ARCHITECTURE.md`, ADR 0006 status, ADR index, `AGENT_ROUTES`,
`runtime.md` locator `bind_internal` (был ошибочно отнесён к `mc-server`) согласованы;
старый план помечен закрытым. Независимый read-only review вернул `changes` с 4 findings
— все исправлены: R1 CP-ARCH-020 требует dependency cut (`use super::*`, packet-типы в
`survival.rs:34-39`, `PreparedStorageBatch`, `InventoryRuntime` в `storage.rs:65`) до
move; R2 порядок shutdown — final save (`checkpoint.rs:557-570` зовёт `writer.flush()`)
при живом journal writer, join после; R3 write-behind для обычных decisions (ADR
0005:199-213, `persistence.rs:721-754`, `world_journal.rs:540-548`) не усиливать; R4
четвёртый stream — plugin-storage journal (`storage.rs:67-69,590-607`). Cargo/клиент
не запускались — дерево кода не менялось.

**Графическая проверка по запросу владельца (2026-09-28, в обход defer).** Все real-client
профили зелёные на текущем dirty tree: `core-client` `20260928T100623-core-client-lwzg423e`,
`inventory` `20260928T101148-inventory-d5s1g__s`, `replay` ×4 (`20260928T101850`,
`T101946`, `T102018`, `T102049`), `playable` (20-мин цикл)
`20260928T110053-playable-ijau2fck`, `loader-live` neoforge `20260928T105255-loader-live-5_ik2x0p`,
fabric `20260928T112702-loader-live-3624kp28`, forge `20260928T112844-loader-live-61a__kzr`;
`java` `20260928T110039-java-ur3n1ulj`, `fixture-check`, `harness-check`, `fmt`,
`code-health` passed. Причины первых падений (все не от рефакторинга): 5 устаревших
standard-pack компонентов против WIT из `9889036e` (пересобраны `build_standard_pack.py
--write`); Loader live fixture был урезан под старый CI pin в `32a50c9c` (возвращены
`input_bindings`, `sounds`, tone-генерация; тоны 1.9 s из-за 2 s лимита Loader; окна audio
gate 0.1+0.25 s); playable использовал захардкоженные recipe display id, устаревшие с
embed-all `5804984e` (теперь поиск по результату, shaped рецепт в приоритете). `replay`
`loaded=0` (`20260927T071452`, `20260928T094722`) на изолированном прогоне не
воспроизвёлся (4/4 pass); причина не установлена, вероятно нагрузка от параллельных процессов.
`code-health`: удалены fail-only LOC-бюджеты gateway-функций и форматозависимая проверка
`matches!(\n name, "minecraft:air"` (противоречили §CP-080 плана); passed `20260928T113320`.

**Закрытый source/tooling CP-REFACTOR-060 (2026-09-27).** Закрытый map
`.analysis/codex-logs/cprefactor-060/plan-map.json`: `regional.rs` разбит на
ownership/handle/coordinator/store и domain tests; `chunk_stream.rs` — на
planning/inflight/prepare/publication и tests; `xtask/main.rs` оставил CLI
dispatch, structural rules в `code_health.rs`. Остатки `play` и смешанные
session/simulation test roots — документированный no-cut, без LOC-only переносов.
ADR 0005/0006 обновлены. `mc-entity --lib` 633 pass / 6 ignored,
`mc-net --lib play::chunk_stream` 78 pass; native L2 `fmt`, strict workspace
Clippy, all-target workspace tests и `code-health` passed (receipts
`.analysis/validation/20260927T072417-fmt-nm3xr9ft`,
`20260927T072421-clippy-pwjpykt8`, `20260927T072435-test-xh7nzeq3`,
`20260927T072020-code-health-01l9l6if`). TCP generated-world spawn window
pass (`20260927T072310-test-2jmo7ogb`). Independent review pass, 0 findings
(`.analysis/codex-logs/cprefactor-060/review.json`). Graphical `replay`
**failed** after Play: `loaded=0`, client chunk load timeout, movement
`DestinationUnloaded` (`20260927T071452-replay-acd9ai9v`); не считать
реальную клиентскую приёмку зелёной. Первоначальные `code-health` и L2
Clippy failures исправлены, повторены только затронутые gates. Receipt
`.analysis/codex-logs/cprefactor-060/receipt.json`.

**Закрытый source/tooling CP-REFACTOR-070 (2026-09-27).** Закрытый map
`.analysis/codex-logs/cprefactor-070/plan-map.json`: `MinecraftScenarioClient`
4767 → 3590 (state/block/entity/container delegates), playable root 7725 → 411
(чистый router) + 14 stage owners, тесты разнесены на 12 contract suites +
router (монолит удалён), `driver.py` 1264 → 862 (colony extracted), dead
`PlaceLoaderBlock` route удалён (ADR 0010 + runtime.md). `run java` passed
(`20260927T180415-java-7xlsqa4g`), `run harness-check` passed
(`20260927T180654-harness-check-ofz9nbla`), `run scenario
inventory-storage-recovery` passed
(`20260927T180655-inventory-storage-recovery-w9i2j4vr`), mc-script 164 /
mc-net router 6. Independent review pass, 0 findings
(`.analysis/codex-logs/cprefactor-070/review.json`). Refused-port driver smoke
остаётся fail-closed `failed draft`, не pass. Receipt
`.analysis/codex-logs/cprefactor-070/receipt.json`.

**Закрытый source CP-REFACTOR-050 (2026-09-27).** Inventory prepare/commit,
window-0/cursor/crafting и simulation dispatch имеют одну authority; без доказанного
mutable mirror они не резались. Close policy перенесена из ingress в
`play/containers.rs::handle_client_close`; creative-slot decode/delegate —
в существующий container ingress, policy осталась у `session::handle_creative_mode_slot`.
40 owner-local тестов перенесены без смены assertions: natural-mob despawn 7, sleep 7,
prepared cache 12, simulation queue 14. Independent review PASS
(`.analysis/codex-logs/cprefactor-050/review.json`). L2: `clippy`/workspace
`test` passed (`20260927T054727`, `20260927T055215`); strict Clippy повторён
после нового TCP теста (`20260927T062623`), `fmt`/`code-health` passed
(`20260927T062625`, `20260927T062630`); TCP crafting close и creative slot
write/refusal passed (`20260927T055006`, `20260927T062453`). Первоначальный
`correctness` прервался на Clippy warning; исправлен, повторён только failed gate.
Graphical `inventory` **failed before client scenario** на startup stale
`basic-economy` wasm (`20260927T050253`), client-visible сохранность не доказана.
Receipts `.analysis/codex-logs/cprefactor-050/receipt.json` и `plan-map.json`.

**CP-REFACTOR-040 (2026-09-26/27; core sub-slices 1-6, 8, 9 и sibling source-фаза 7 — закрыты и reviewed).** Core:
(1) `survey.rs`; (2) `certificate.rs` + тесты; (3) `warehouse.rs` + тесты; (4) `construction.rs` + тесты —
`settlement.rs` 3544 → 2348, `settlement_tests.rs` 6088 → 4237; (5) `resident_order_tests.rs` 7614 → 1012 + четыре
контрактных модуля (75 тестов) и fixture-capability migration; (6) `mc-script`: `lib_tests.rs` (60), `runtime.rs` (121),
`command_admission.rs` (1071); (8) `route_authority.rs` (route authority); (9) boundary closure — `ScriptBoundary` /
`ScriptHostEndpoint` / `script_boundary_pair` / coalesced-tick helpers / `target_is_live` переехали в тот же модуль,
route-типы снова private, тесты используют узкие `#[cfg(test)]` helpers — `lib.rs` 8394 → 3981. Sibling
`sources/solaris-settlements`: `lib_tests.rs` (51), `registry.rs` (299), `legacy_commands.rs` (336), `warehouse_policy.rs`
(161), `command_dispatch.rs` (260) — `lib.rs` 7609 → 4823. Проверки: **L2 `correctness` passed** (`.analysis/validation/20260927T041757-correctness-j8s2rk9y`), `run fmt`, `run code-health`, `cargo check
--workspace --all-targets` clean, `mc-script --lib` 164, `mc-plugin-host --lib` 9, `mc-net --lib script::storage::` 164,
`resident_order_tests` 75, `construction_tests` 17, `certificate_tests` 4, sibling package 90 passed / 0 warnings, harness
unit-тесты 21, catalog ids 4/4. Reviews: 1 `pass`, 2 `changes`→fixed, 3 `pass`, 4 `changes`→fixed, 5 `changes`→fixed,
6 `pass`, 7 `pass` ×2, 8 `changes`→fixed, 9 `changes`→resolved как задокументированное исключение (восемь accessor-подстановок
из sub-slice 6, поведение подтверждено). Открыто внешне: `settlement-cargo` fail-closed на component drift `71562f94…` →
`5288066d…`; `--write` требует авторизации владельца. Sibling pending-operation pipeline оставлен в корне пакета как cohesive owner (решение с доказательствами в receipt).
Receipt `.analysis/codex-logs/cprefactor-040/`.

**Закрытый CP-REFACTOR-030 (2026-09-26).** Warehouse-binding вынесен в
`crates/mc-net/src/script/storage/settlement/warehouse.rs` (526 строк) с тестами в
`settlement/warehouse_tests.rs` (260) как child-модуль `settlement_tests` через `#[path]`
(фикстура переиспользована, без копии). `settlement.rs` остался корнем (3544 строки: ledger
authority, shared `apply`, survey/construction/certificate). Поведение неизменно (pure move,
подтверждено независимым review token-by-token); видимость минимальна
(`pub(in crate::script::storage)` для двух типов, `pub(super)` только у нужных parent-у
функций). Catalog ids `warehouse-bind-manual`/`warehouse-bind-refusals` указывают на новый
модуль. Стоимость: hops 3→2, review churn 10128→786 строк; focused-цикл не ускорился
(сборка по-прежнему ~50–60 с). Receipt `.analysis/codex-logs/cprefactor-030/`; review
`changes` → 3 findings исправлены (неверные source pointers у reservation-сценария,
устаревший verification selector и line anchors, лишняя `pub(super)` у трёх child-only
хелперов).

**Закрытый CP-REFACTOR-020 (2026-09-26).** Один каталог существующих проверок
`tools/harness/scenarios.py` (typed records: profile+selector+tests+evidence kind+
prerequisites+fixture+sources+proves/limits), `explain` без side effects и
`run scenario <id>` через единственный run path. Fail-closed: неизвестный id, лишние
аргументы, отсутствующий тест, нет engine counts, zero-selected/ignored-only. Receipt
`details.named_scenario` несёт counts, версии репозиториев, cwd каждой команды, границы
доказательства и привязанный `scenario_id`/manifest. Receipt:
`.analysis/codex-logs/cprefactor-020/`; review `changes` → 3 findings исправлены
(replay identity binding через `ProfileContext`, `prepared` wording, §7.3 evidence fields).

**Закрытый CP-REFACTOR-010 (2026-09-26, docs-only).** Написаны фактические карты
[architecture/runtime.md](architecture/runtime.md) (модули, очереди/tasks, два реальных
писателя inventory), [state-ownership.md](architecture/state-ownership.md) (state → owner →
writer → record → readers → boundary) и [mutations.md](architecture/mutations.md) (5 карточек:
warehouse bind, warehouse deposit, owned/compound crash recovery, window click/crafting,
harness run), плюс пилотный контракт
[architecture/pilot-warehouse-binding.md](architecture/pilot-warehouse-binding.md) для
CP-REFACTOR-030. Receipt `.analysis/codex-logs/cprefactor-010/`; независимый review вернул
`changes` (6 findings: cfg(test)-helper в диаграмме, `SimulationHandle` vs `SimulationOwner`,
строка DTO-валидации, порядок durable decision в card 3, две ветки отказа в card 4, пропущенный
исходный test-модуль в write set) — все исправлены; `codegraph sync .` exit=0.

**Закрытый CP-REFACTOR-000 (2026-09-26, docs-only).** Зафиксированы tree identities трёх
репозиториев, CI source pins и 7 deployed `plugin.wasm` siblings; история `MEMORY.md`
разнесена по `docs/memory/log-09…17` verbatim (5067 строк, только относительные ссылки и
заголовки частей); продуктовый roadmap P0–P8/CP-001–096 перенесён в
[`archive/PRODUCT_ROADMAP_WASM.md`](archive/PRODUCT_ROADMAP_WASM.md) (verbatim, sha256 внутри,
выбор `next` из него запрещён); три контрольные developer задачи измерены; доменные ссылки
согласованы (Luau-эра помечена историей, 25 профилей). Receipt:
`.analysis/codex-logs/cprefactor-000/receipt.json`. Независимый read-only review вернул
`changes` с двумя findings (конкурирующий `next` в плане, склейка plugin-result delivery с
client projection в ADR 0010) — оба исправлены; второй reviewer по правилу не запускался.

**Измеренный baseline focused-цикла** (`mc-net` lib): `python3 -m tools.harness run test --
-p mc-net --lib <filter>` = 58.3 s сборки (`mc-script`, `mc-plugin-host`, `mc-net`) + 0.4 s
исполнения 3 тестов; тёплый повтор без изменений — 0.6 s; 2 crash-теста — 3.5 s. Стоимость
цикла определяется сборкой, а не тестами: дробление `tests.rs` по модулям само по себе rustc не ускоряет.

## CP-049 (manual building) — открыт, ждёт очереди

Manual warehouse binding и building certificates в коде **уже есть**: writer —
`bind_manual_warehouse` в `crates/mc-net/src/script/storage/settlement.rs`, behavior —
`crates/mc-net/src/script/storage/settlement_tests.rs`; `SettlementLedger::binding_position`
и `warehouse_for_position` возвращают `Option`, а неразрешимый authored binding
пропускается cross-kind lookup, не блокируя последующие bind. Полный acceptance BUILD-05/07
**не доказан**: manual building capabilities, реальный доступ/станции, invalidation после
разрушения, repair, unload/restart, foreign owner и объяснение отказа. Пропуск —
`.analysis/cp049-resume-20260926.md` (локальный provenance; его прежний `next` не
исполнять). Возврат — по CP-ARCH-070: карточка `construction / manual building
recognition` с реальными файлами, owner, gates и первым недостающим outcome.

## Границы и blockers

- **Feature freeze снят владельцем (2026-10-01).** CP-049–096 разблокированы. Правило владельца:
  в ядре НЕТ плагинов — ни доменного кода поселений/экономики/городов/claims, ни их тестов;
  ядро = движок + широкий переиспользуемый plugin API, домен живёт в `../solaris-default-plugins`.
  План расширения поселений: `/home/kaiserroman/Downloads/SOLARIS_SETTLEMENTS_LONG_TERM_ADDITION_SPEC.md`
  (встраивается в канонические спеки плагина). Частичная выгрузка cargo остаётся частичной.
- **Graphical acceptance.** Все real-client профили прошли 2026-09-28 (см. ниже) на
  дереве до CP-ARCH; каждый CP-ARCH, трогающий client-visible surface, называет свой
  реальный клиентский gate (§4 плана). Известный проваленный owner-сценарий остаётся
  `blocked` до нужного rerun; зелёные Rust/TCP/Java гейты его не понижают и не заменяют.
- **Commit/push не разрешены.** Работа фиксируется path-limited diff от `base_trees` выше.
- Открытые runtime-пробелы CP-047/CP-048 (housing/food pause triggers, guardian-death
  reissue, relocation identity model) и остальной незакрытый контекст — в `log-17`/`log-16`,
  а не здесь.

## Как этим пользоваться

- Маршрут выбирает `route` активного чекпоинта: [AGENT_ROUTES.md](AGENT_ROUTES.md).
- Активная программа и очередь CP-ARCH: [CORE_ARCHITECTURE_PLAN.md](CORE_ARCHITECTURE_PLAN.md);
  решение: [ADR 0011](decisions/0011-core-crate-topology-and-threading.md).
- Команды, prerequisites, receipts и диагностика: [AGENT_TOOLING.md](AGENT_TOOLING.md).
- Нормативные границы: [ARCHITECTURE.md](ARCHITECTURE.md) (target) и owning ADR в `decisions/`;
  фактическое устройство: [architecture/runtime.md](architecture/runtime.md),
  [architecture/state-ownership.md](architecture/state-ownership.md),
  [architecture/mutations.md](architecture/mutations.md).
- Продуктовые CP-ids и acceptance-контекст P0–P8/CP-001–096 (historical provenance из
  `Downloads`, редакция 2026-09-16, не очередь задач):
  [archive/PRODUCT_ROADMAP_WASM.md](archive/PRODUCT_ROADMAP_WASM.md).
- Готовность и приёмка: [DEFINITION_OF_DONE.md](DEFINITION_OF_DONE.md),
  [VALIDATION_LEDGER.md](VALIDATION_LEDGER.md).

## Архив чекпоинтов

| Файл (историческое, не startup context) | О чём |
| --- | --- |
| [`memory/log-01-villages-and-content-source.md`](memory/log-01-villages-and-content-source.md) | Ранняя разведка деревень, границы фич, источник ванильного контента (кэш + импортёр). |
| [`memory/log-02-settlement-c1-and-handover.md`](memory/log-02-settlement-c1-and-handover.md) | Профили поселений, закрытие ядра, путь записи склада (C1), снапшот передачи. |
| [`memory/log-03-gameplay-and-loader.md`](memory/log-03-gameplay-and-loader.md) | Постройки в рельефе, мобы/блуждание, связка плагин↔C4, бой C4, cutover Loader wire. |
| [`memory/log-04-releases-and-handoff-fixes.md`](memory/log-04-releases-and-handoff-fixes.md) | Релизы v0.0.6, починка тестов, серия handoff-issue (сундуки, вёдра, рыба, двойной сундук). |
| [`memory/log-05-performance-and-worldgen.md`](memory/log-05-performance-and-worldgen.md) | Проверенные оптимизации и верификация мира (реки, биомы, берега, свет). |
| [`memory/log-06-checkpoints-09-12-to-09-14.md`](memory/log-06-checkpoints-09-12-to-09-14.md) | Чекпоинты 09-12…09-14: tab-list, реки, деревни+редстоун, сигналы shutdown, feature executor A1. |
| [`memory/log-07-village-generation.md`](memory/log-07-village-generation.md) | Feature executor A2 (деревья), активация ванильной генерации деревень, village solver + decor lane. |
| [`memory/log-08-audit-closeout-and-inhabitants.md`](memory/log-08-audit-closeout-and-inhabitants.md) | Закрытие аудита ядра и жители деревень из маркеров частей (закрытый чекпоинт 09-15). |
| [`memory/log-09-live-state-r0-r1-and-core-warehouse.md`](memory/log-09-live-state-r0-r1-and-core-warehouse.md) | Живое состояние, R0/R1-A/R1-B, core warehouse CP-001…CP-003. |
| [`memory/log-10-wasm-plugin-host-p0.md`](memory/log-10-wasm-plugin-host-p0.md) | WASM plugin host P0: memory bound, hostile-guest evidence, composition root. |
| [`memory/log-11-wasm-audit-p1-p3-p4-p5.md`](memory/log-11-wasm-audit-p1-p3-p4-p5.md) | WASM audit, P1 contract/runtime, P3 storage/inventory/menus, P4, P5 blocked. |
| [`memory/log-12-p5-p8-and-component-checkpoints.md`](memory/log-12-p5-p8-and-component-checkpoints.md) | P5 accepted…P8 cutover, CP-001…CP-007 component era, architecture review. |
| [`memory/log-13-release-close-and-cp008-014.md`](memory/log-13-release-close-and-cp008-014.md) | v0.0.7 close, CI repair, CP-008…CP-014 agricultural/work checkpoints. |
| [`memory/log-14-v0-0-8-release-candidate.md`](memory/log-14-v0-0-8-release-candidate.md) | v0.0.8 release candidate. |
| [`memory/log-15-deferred-acceptance-source-corrections.md`](memory/log-15-deferred-acceptance-source-corrections.md) | CP-014 R1 chain и серия source-audit правок с отложенным client acceptance. |
| [`memory/log-16-headless-plugin-checkpoints-cp030-046.md`](memory/log-16-headless-plugin-checkpoints-cp030-046.md) | CP-030…CP-046 headless: inventory kernel, WIT handles, durable payment, combat, squads. |
| [`memory/log-17-latest-checkpoints-cp047-048.md`](memory/log-17-latest-checkpoints-cp047-048.md) | CP-047 combat recovery и CP-048 birth record layer. |
