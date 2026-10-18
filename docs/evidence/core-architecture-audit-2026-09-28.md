# Аудит архитектуры ядра Solaris — 2026-09-28

**Дерево:** `4865344e` (после закрытия CP-REFACTOR-100), dirty только чужие
`WATCHDOG.yml`, `tools/harness/{loader,profiles,scenarios}.py`.
**Метод:** семь независимых read-only обследований по срезам (ingress, simulation
owner, regional entity, plugin storage, composition/world, tests/tooling,
threading) плюс сверка с нормативными документами. Полные отчёты с полными
цитатами — `.analysis/codex-logs/cparch-000/*.md` (ignored). Здесь — только
findings, у каждого нарушенный инвариант, `file:line` и следствие. Cargo/тесты
не запускались: дерево не менялось.

**Решение по итогам:** [ADR 0011](../decisions/0011-core-crate-topology-and-threading.md)
и программа [`CORE_ARCHITECTURE_PLAN.md`](../CORE_ARCHITECTURE_PLAN.md).

## 1. Вердикт в одном абзаце

Механизмы ядра — хорошие: bounded очереди, single-writer owner c монотонной
последовательностью команд, регионные OS-lanes с детерминированным порядком,
conditional state/token commit в `mc-world`, отдельный journal-поток с fsync,
чистые kernels (`mc-physics`, block placement/break, survival damage, explosions),
изолированный Wasmtime host. Архитектурная проблема — не в механизмах, а в
**границах владения**: весь движок (simulation, player state, containers,
persistence, journal, tick, settlement/resident domain, plugin storage, control
plane) живёт в `mc-net` и публично экспортируется оттуда; authority
(`SimulationOwner`) зависит обратно на адаптер (`SessionRegistry`,
`OutboundCommand`); адаптеры владеют gameplay policy; у player inventory два
писателя; composition root раздвоен; production-бинарь зависит от test harness.
Cargo эту направленность не защищает — только текстовые tripwires `code-health`.
Предыдущая программа (CP-REFACTOR) сознательно дробила файлы, не трогая
ownership (§5.4 старого плана) — и это она честно зафиксировала.

## 2. Findings

Категории: **A** ownership/authority, **B** coupling/compile-unit, **C**
threading/waiting/durability, **D** dead/duplicate. Severity: `blocking`
(мешает целевой архитектуре или угрожает сохранению), `major`, `minor`.

### A. Ownership / authority

| # | Sev | Инвариант | Факт и evidence | Следствие |
|---|---|---|---|---|
| A1 | blocking | Один writer на состояние (ADR 0004; `ARCHITECTURE.md:99-100`) | Player inventory: owner-путь `SimulationHandle::commit_player_inventory` (`play.rs:4277-4280` → `simulation.rs:6432-6452` → `session/player_state.rs:116-200`) **и** прямые писатели в connection task: `commit_session_owner_script_player_inventory` (`play.rs:3973-3990`), `commit_session_owner_loader_item_grant` (`play.rs:3993-4010`), compound storage gate (`session/script_inventory_transaction_endpoint.rs:128`). Все под `player_persistence` mutex, owner отвергает stale plan — потеря обновления не доказана | Любое новое правило inventory нужно реализовывать в двух путях; порядок команд owner обходится |
| A2 | blocking | Authority не зависит от адаптера; simulation usable без TCP (`ARCHITECTURE.md:24-37`) | `simulation.rs:36-65` импортирует `SessionRegistry`, `SessionId`, `OutboundCommand`, visibility types; `process_commands_with_world_views` принимает `&SessionRegistry` (`simulation.rs:4561-4619`) и берёт journal через `sessions.world_chunk_journal()`; `session.rs:24-26` импортирует `SimulationAuthority` обратно | Simulation нельзя вынести или использовать headless без всего session-слоя; цикл simulation ↔ session |
| A3 | major | Session — индекс соединений, не gameplay authority (ADR 0004:90-94) | `SessionRegistryInner` (`session.rs:455-523`) = membership + canonical player records (`:507`) + active shields/sleep (`:510-514`) + world time/weather/rules atomics + regional façade (`:636-639`) + container registries + prepared chunk cache + journal handle. Admin game rules пишутся напрямую (`command_execution.rs:100-132,315-317`) | Один фасад — и session index, и cross-domain authority API; всё, что нужно player state, тянет session |
| A4 | major | Адаптер не владеет gameplay policy (`ARCHITECTURE.md:29`; ADR 0006:21-23) | Menu policy в `play.rs`: generic click/window-0 `5959-6302`, crafting `2686-2858`, enchanting `2966-3243,6327-6441`, recipe book `3819-3935`; attack cost `7200-7231`; shield `7517-7567` пишет `set_active_shield` напрямую; bed/sleep `11489-11661` → `sleep.rs:489-524` пишет world time **мимо** `SimulationOwner::advance_world_time` (`simulation.rs:3824-3835`) | Второй writer world time; policy размазана между root и focused-модулями, которые уже существуют (`containers::*`, `combat`, `session/sleep`) |
| A5 | major | Spatial policy у spatial owner (`ARCHITECTURE.md:26-30`) | Fall damage: сеть копит descent в `entity_owner.rs:11-13,361-390` и считает damage в `entity_combat.rs:1339-1363`, при этом ECS сам ведёт `fall_distance` (`mc-entity/runtime.rs:4236-4264`). AI planners hostile/passive/villager/dragon — в `mc-net/play/session/{hostile_authority,passive_mobs,villager_population,dragon_authority}.rs`, kernels — в `mc-entity` (`dragon_26_1_2.rs`, `villager_26_1_2.rs`) | Два представления одного состояния; поведение сущностей нельзя тестировать без `mc-net` |
| A6 | major | Router только диспетчеризует (ADR 0006:21) | `script/router.rs:478-623`: overworld-only, height/horizontal bounds, default block resolution, protection fence — до отправки в owner | Правила блоков живут в двух местах |
| A7 | major | Durable transaction coordinator — владелец, не транспортный модуль | World-chunk WAL + inventory receipts — `mc-net/play/world_journal.rs:1-20,477-550`; открывается и ставится на `WorldStorage` и `SessionRegistry` из `mc-net::bind_internal` (`server.rs:2737-2772`). `mc-world` при этом чист (`mc-world/Cargo.toml:14-22`, `resident.rs:1442-1500,1586-1630`) | Durability-контракт мира принадлежит `play` |
| A8 | minor (документировать) | Явный durability-контракт на операцию | Четыре независимых durable stream: world-chunk WAL (`world_journal.rs:57-61`; обычные decisions — write-behind, `flush` только при inventory receipt `:540-548`), regional entity WAL (`persistence.rs:713-788`; `record_commits` возвращает после `send`, без ожидания sync `:721-754` — по ADR 0005:199-213), plugin-storage journal (`storage.rs:67-69`, `commit_batch` `:590-607` с sync `:1519-1542` до projection), player `.dat` checkpoint (`persistence.rs:1434-1529`, `checkpoint.rs:814-819`). Обычный chest click **не** WAL-backed (`simulation.rs:902-913`; `mc-world/storage/block_edits.rs:64-97`), только receipt-bearing маршруты | Это осознанная политика (нет universal transaction), но нигде не зафиксирована как контракт с acknowledgement на stream; новая composite-операция должна явно выбирать journal |

### B. Coupling / compile unit

| # | Sev | Инвариант | Факт и evidence | Следствие |
|---|---|---|---|---|
| B1 | blocking | `mc-net` — connection adapter; `mc-server` — composition root (`ARCHITECTURE.md:36`) | `mc-net/src/lib.rs:19-44` объявляет `play`, `script`, `settlement`, `control_plane`, `autoscale_soak`, `memory_pressure`, `chunk_pipeline`, `dirty_flush`, `lock_policy/metrics`, `runtime_entity_budget`; `:51-105` реэкспортирует всё это; `Cargo.toml:19-27` зависит от entity/physics/world/worldgen. 277 K строк, 430 файлов | Направление зависимостей защищено только текстовыми tripwires; любая правка движка = пересборка всего `mc-net` (~58 s по baseline CP-REFACTOR) |
| B2 | blocking | Один composition root | `mc-server/main.rs::serve` (`:152-520`) готовит данные/мир и зовёт `mc_net::bind`; `mc-net/server.rs::bind_internal` (`:2666-3054`) сам строит `SessionRegistry`, regional runtime, journals, simulation channel, settlement runtime, plugin storage, inventory recovery. `runtime.md:12-14` ошибочно относит `bind_internal` к `mc-server` | Wiring движка живёт в транспортном crate |
| B3 | major | Production не зависит от test tooling | `mc-server/Cargo.toml:27` — `mc-test-harness` в `[dependencies]`; используется только `registry_capture::capture_registry_payloads_from_jar` (`content_import.rs:848`) и `parity::parse_java_major_version` (`:915`). Тяжёлые deps harness'а — dev-only (`mc-test-harness/Cargo.toml:45-53`) | Content-import код без production-владельца; harness — в графе production build |
| B4 | major | Gameplay-домен не в транспортном crate | Settlement/resident домен (ledgers, construction, survey, certificate, morale, order execution) — `mc-net/src/script/storage/**` (`resident_order_execution.rs:691`, `resident_morale.rs:73-88`, `settlement.rs:275,1213`); `InventoryRuntime` (`world_inventory.rs:109`) держит `SessionRegistry` + world journal + settlement + resident adapters и compound inventory commit (`:371`) | Один struct координирует четыре домена; правила residents нельзя тестировать без session |
| B5 | major | Нет тестовых roots в production compile unit без причины | `mc-net` без `tests/`; giant roots под `#[cfg(test)]` в `src/` (`play.rs:14732`, `session.rs:2690`, `simulation.rs:8338`, `regional.rs:4125`): ≈245/120/172 тестов на builders `register_test_session` (`session/tests.rs:3007-3026`), `test_block_storage` (`simulation/tests.rs:482-535`), `RegionalOwnerCoordinator::from_store` (`regional/tests.rs:445`). Тесты используют `super::*` и crate-private state | `cargo test -p mc-net --lib` — один compile unit со всем движком; вынос в `tests/` невозможен без публичной границы |
| B6 | minor | Один каталог packet layouts (ADR 0002) | `wire_entities.rs:53-88`: локальный `MoveEntityRot` с id `0x38` и ручным encode вне `mc-protocol` | Второй packet catalog |
| B7 | minor | Один shared value type на понятие | `mc-entity/projectile_26_1_2/lifecycle.rs:26,42` свои `EntityId`/`BlockStateId`; `arrow.rs:22-26` `BlockPosition` дублирует `mc_world::BlockPos`; `EntityItemStack` (`mc-entity/lib.rs:178-181`) повторяет поля `mc_data::ItemStack`. `mc-domain` — 3 enum (`lib.rs:7,38,76`) | Неявные конверсии; `mc-domain` как «shared value layer» не состоялся — и раздувать его не надо. **CP-ARCH-010:** projectile `EntityId` заменён на `mc_entity::EntityId` (pure duplicate, конверсии `projectile_entity`/`session_entity` удалены). `BlockStateId`/`BlockPosition` оставлены: они serde-типы persisted kernel state, а `mc_world::{BlockStateId,BlockPos}` без serde — замена потребовала бы новую зависимость `mc-world → serde` ради minor-дубля. `EntityItemStack` — не pure duplicate (своё component-представление), оставлен |
| B8 | minor | Tripwires защищают инварианты ADR | `xtask/code_health.rs` — текст/путь/anchor сканеры (`:1687-1719`, Cargo читается построчно, Python — narrow grammar); нет проверки single-inventory-writer, socket-write-under-lock (ADR 0006:159-161), import-DAG/SCC | Ложная уверенность; реальная защита направления — только Cargo-граница |
| B9 | minor | Один schema owner для guest contract | WIT (`residents.wit:430`, `commands.wit:496`) и `mc-script` DTO (`resident_order_operations.rs:433`, `lib.rs:3259`) авторятся параллельно, конверсия вручную (`mc-plugin-host/domain_residents.rs:422`, `adapter.rs:98`). `mc-script` зависит от tokio (`Cargo.toml:18`, `route_authority.rs:444,845`) | Три точки синхронизации на изменение контракта; «runtime-independent» = VM-independent, не runtime-neutral — это надо назвать, а не чинить |

### C. Threading / waiting / durability

| # | Sev | Инвариант | Факт и evidence | Следствие |
|---|---|---|---|---|
| C1 | blocking | Медленная работа изолирована от tokio workers (`ARCHITECTURE.md:119-122`) | `SimulationOwner` живёт **внутри** tokio-задачи ticker (`server.rs:1758-1762`); tick синхронно зовёт `tick_owned_regions` (`entity_ticker.rs:511-514` → `entity_simulation.rs:1004-1009` → `regional/handle.rs:2375-2385` `send` + блокирующий `recv()`), coordinator joins lanes (`coordinator.rs:3598-3635`) | Tokio worker заблокирован на всё время регионального тика; tick-поток и I/O runtime делят пул |
| C2 | major | Не держать session lock, ожидая regional owner (ADR 0004:402-403) | `lock_session_entities` (`session.rs:893-896,1688-1698`) + hostile commit `hostile_authority.rs:2330-2360` делает CAS через `entity_owner.rs:1208-1217` → `handle.rs:689-706` blocking recv под session guard | Все session-операции сериализованы за задержкой lane |
| C3 | major | Sync I/O не на async task | Plugin storage actor — `tokio::spawn` (`storage.rs:1889-1899`); `append_frame` пишет и `sync_all` синхронно (`:1519-1542`), compaction (`:1505-1516`). В него из actor'а входят **все** durable вызовы, не только `commit_batch`: `compare_and_swap_durable` (`:2059`), `delete_durable` (`:2130`), `acknowledge_result` после delivery (`:2120`; replay `:2575-2577`), `acknowledge_operation` (`:2579`) | fsync блокирует worker на commit **и** на ack; offload одного `commit_batch` не закрывает finding |
| C4 | major | Sync I/O не на connection task | `play::handle` (`play.rs:1692-1717`) при входе зовёт `load_player_state_for_login` (`:1745-1753`) → `persistence.rs:1323-1332,2555-2593` `File::open`/`read_exact`/gzip | Медленный playerdata FS стопорит worker до Play |
| C5 | major | Final save не отменяется по таймеру | `main.rs:33` 6 s; `:672-703` `timeout` **оборачивает** pinned `serve_and_save` и дропает future; внутри connection drain 5 s + ticker drain 5 s (`server.rs:82,88`), final save только после `serve` (`server.rs:3123-3126`); ticker abort (`server.rs:2135-2157`) убивает задачу, которая делает `persist_inhabited_time_tail` и `simulation_owner.shutdown()` (`entity_ticker.rs:233-258`) | Медленный shutdown может отменить финальное сохранение; сумма внутренних дедлайнов > внешнего |
| C6 | major | Каждый owner joined при остановке | `run_storage_actor` spawn без JoinHandle (`storage.rs:1889-1899`); `wait_stopped` есть (`:1959-1969`), но `BoundServer::serve` shutdown (`server.rs:1893-1934`) его не зовёт | Процесс может завершиться, не дождавшись durable-actor |
| C7 | minor | Bounded admission (`ARCHITECTURE.md:105-117`) | Dashboard: каждый accept → `tokio::spawn` без лимита/JoinSet (`dashboard.rs:218-226`), parent abort не join'ит детей (`main.rs:573-574`) | Unbounded fanout на диагностическом порту |
| C8 | minor | Не держать global guard через wait на другого owner | `warehouse_resident.rs:40` берёт `save_coordinator`, `:99` ждёт `resident_entity_snapshots` (spawn_blocking → regional handle) | Head-of-line для всех save/world decisions |
| C9 | minor | Fallback world read не под async lock с sync I/O | `simulation.rs:4777-4805` `world.lock().await` + `process_batch`; fallback путь может открыть Anvil region синхронно (`mc-world/storage.rs:1074-1125`) | Диск под async lock мира (только non-resident fallback) |
| C10 | minor | Lock order документирован и проверяем | `lock_policy.rs` (net и entity) — только poison policy (`mc-net/lock_policy.rs:50-90`, `mc-entity/lock_policy.rs:41-68`); порядок — локальные комментарии | Deadlock-safety не доказана и не проверяется |

Отдельно: `std::thread::sleep` в production — только Wasmtime epoch watchdog
(`mc-plugin-host/lib.rs:117-139`), это deadline-механизм для guest, а не scheduler;
`std` guard через `.await` в проверенных путях не найден; `unbounded_channel` в
production не найден; все MPSC bounded (simulation 1024/256, script 1024/256,
storage 256+256, lanes 64, journal 64, chunk result 64, memory 1, tick metrics 1).

### D. Dead / duplicate

Мёртвых production-путей не найдено ни в одном срезе. Дубли: A1 (writer), A5
(fall distance), B6 (packet), B7 (value types), B9 (schema). Дубли gameplay
kernels (damage/placement/break/food/explosion) **не** найдены — везде один
focused planner и owner commit (`block_placement.rs:84-100` +
`use_item_on_adapter.rs:2041-2044`; `block_break.rs:349-360,549-560` +
`simulation.rs:7960-7972`; `play.rs:924-940` + `player_damage_adapter.rs:171-175`).

## 3. Что сохранить как есть

- `mc-world`: без tokio, conditional state/token commit, region-file flush lifecycle (`resident.rs:1442-1500,1586-1630`; `storage/dirty_flush.rs:395-410,501-515`).
- `mc-physics`: pure kernel, один runtime dep (`Cargo.toml:14-15`, `lib.rs:693-738`); вызывается из entity и net.
- `mc-entity::regional`: per-region ECS `World` (`runtime.rs:366-418`), lanes 64/OS thread (`owner_lane.rs:811,848-870`), lease/epoch/phase (`ownership.rs:35-190`), детерминированный порядок lanes (`regional.rs:1994-2038,2267-2298`), prepare/commit/journal/finalize/rollback (`coordinator.rs:4194-4438`), fatal unknown outcome.
- `mc-protocol`/`mc-nbt`: typed codecs, budgets (`mc-nbt/lib.rs:188-260`).
- `mc-server::startup_data`: данные до мира (`main.rs:225-239,287-327`).
- Simulation queue: bounded, `try_reserve`/`reserve`+timeout, request sequence (`simulation/queue.rs:25-27,371-385,418-423`).
- Journal writer: один поток, 64, fsync до reply для receipt-bearing decisions/`flush`/`checkpoint_through`, write-behind для обычных decisions (ADR 0005), join на drop (`world_journal.rs:37,205-257,540-548,899-989`).
- Chunk pipeline: CPU/I/O semaphores, старт с 1 worker, adaptive (`chunk_pipeline.rs:49-61,378-402`).
- Outbound: reliable/best-effort, bounded retry, shed slow client (`session/outbound.rs:242-244,1216-1314`).
- `arc_swap` publications; `entity_owner.rs` как узкий adapter без mutable ECS refs; `EntityTrackingMotion` — компактный факт, не пакет.
- Wasmtime: два store (configure/init), fuel/epoch/memory limits, host OS thread, join на stop (`mc-plugin-host/host.rs:469-517`).
- Focused kernels и разбиение `chunk_stream/*`, `ingress/*`, `play/tests/**`.
- Harness: один writer receipt, event-driven readiness (`runtime.py:153-205`); тесты — channels/barriers, без sleep.

## 4. Что из этого следует

Все blocking-findings (A1, A2, B1, B2, C1) — одно и то же: граница
«simulation/application core» существует как соглашение, а не как crate. Пока
это так, `code-health` — единственная защита, и она текстовая (B8). Целевая
форма и порядок работ — ADR 0011 и `CORE_ARCHITECTURE_PLAN.md`.
