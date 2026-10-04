# Solaris current cursor

Живой курсор проекта. Здесь только то, что верно **сейчас**, и карта остального. Историю
держим в [`docs/memory/`](memory/) — это **не** стартовый контекст (`AGENTS.md`); открывай
часть, когда вопрос про ту эпоху.

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
