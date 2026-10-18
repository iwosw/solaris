# Solaris: Developer Experience / Agent Readiness Refactor

**Дата:** 2026-09-26.  
**Маршрут:** `architecture`.  
**Статус плана:** **закрыт 2026-09-28** — все CP-REFACTOR-000…100 выполнены (§12). Преемник: [`CORE_ARCHITECTURE_PLAN.md`](CORE_ARCHITECTURE_PLAN.md) (ownership/crate-граница, которую этот план сознательно не трогал — §5.4). Документ сохранён как история решений и acceptance; `next` из него не выбирать.
**Статус продукта:** `draft`; этот документ не закрывает CP-049 и не повышает readiness.  
**Разрешение:** владелец разрешил реализацию 2026-09-26. Commit/push не разрешены; source/tooling срезы идут path-limited diff от base trees в [`MEMORY.md`](MEMORY.md). Frozen остаётся перечисленное в §4.1.

## 1. Решение в одном абзаце

Приостановить расширение gameplay и выполнить конечную программу упрощения работы с
существующим кодом трёх репозиториев. Сначала сделать короткий и непротиворечивый вход,
карту **фактических** владельцев и каталог уже существующих проверок. Затем провести
один сквозной пилот на warehouse binding и распространить проверенный способ на
settlements, inventory/simulation, крупные native-модули и клиентскую тестовую
инфраструктуру. Разрезать по ответственности, а не по числу строк. Не создавать
новый scenario DSL, второй runner, универсальный transaction framework или набор
пустых crates. Закончить разрешённую source/tooling работу и её проверки, затем
вернуть CP-049 в очередь с его настоящими пробелами. Отложенная владельцем
графическая приёмка остаётся отдельным открытым обязательством, не PASS и не
условием бесконечной заморозки продуктовой очереди; точные два уровня закрытия — §8.

Это крупный рефакторинг существующих границ, а не разрешение переписать сервер.
Его единица — законченный домен с владельцем, входом изменения и проверкой, а не
«ещё 500 строк перенесены».

## 2. Что проверено при подготовке

### 2.1 Исходное дерево и достоверность

Проверены локальные checkout, а не предполагаемое состояние GitHub `main`:

| Репозиторий | HEAD при исследовании | Рабочее дерево до этой задачи |
|---|---|---|
| `solaris` | `9889036ee786dc90d2e44d7aeeb80fa62efe1a5d` | clean |
| `solaris-default-plugins` | `fe265c00463bf535661787d48a8f3c72b5a4564e` | untracked `.analysis/`, `WATCHDOG.yml`, `tools/__pycache__/`; не трогать |
| `solaris-loader` | `06a268ecf710d1066c5bd4a7113d404c5f15013e` | clean |

Измерены tracked-файлы с расширениями `.rs`, `.py`, `.java`; это физические строки,
включая комментарии и тесты, **не** оценка production SLOC и не полный semantic audit.
Значения — baseline для приоритизации, не обещание разбить каждый файл.

| Область | Наблюдение |
|---|---|
| Core | 933 таких файла, 628 907 строк; 234 файла больше 700 строк, 87 — больше 1 500 |
| `mc-net` | 276 810 строк Rust, включая тесты |
| Plugins | 33 таких файла, 28 106 строк; 13 файлов больше 700 строк |
| Loader | 198 таких файлов, 49 014 строк; 11 файлов больше 700 строк |
| Живой `docs/MEMORY.md` | 5 094 строки до добавления нового приоритета, при обещании «держим его маленьким» |
| Python harness | 11 197 строк `.py`; отдельные shell backends в это число не входят |

Крупные точки приложения усилий:

| Файл на исходном дереве | Строк | Почему важен |
|---|---:|---|
| `crates/mc-entity/src/regional.rs` | 22 672 | региональный owner, транзакционные границы; нельзя механически дробить с изменением lock order |
| `crates/mc-net/src/play.rs` | 14 744 | сетевой ingress и оставшаяся orchestration/gameplay-связность |
| `crates/mc-net/src/play/session/tests.rs` | 15 634 | поиск нужной проверки и общие fixtures |
| `crates/mc-net/src/play/simulation/tests.rs` | 12 305 | разные контракты одного owner смешаны в большом test root |
| `crates/mc-net/src/play/simulation.rs` | 8 341 | command surface и порядок применения |
| `crates/mc-net/src/play/chunk_stream.rs` | 8 357 | отдельный крупный state machine, не часть warehouse-пилота |
| `crates/mc-script/src/lib.rs` | 8 394 | DTO/admission/API surface, дорогой поиск контракта |
| `crates/mc-net/src/script/storage/settlement.rs` | 4 040 | sites, structures, survey, warehouse bindings, certificates |
| `crates/mc-net/src/script/storage/settlement_tests.rs` | 6 088 | смешанные behavioral suites и fixtures |
| `crates/mc-net/src/script/storage/resident_order_tests.rs` | 7 614 | большая native-проверка resident lifecycle/work |
| `crates/xtask/src/main.rs` | 4 069 | архитектурные tripwires уже есть; не нужен второй инструмент |
| Plugins: `sources/solaris-settlements/src/lib.rs` | 7 609 | policy, parsing, dispatch и операция/ответ связаны в одном entrypoint |
| Plugins: `sources/integration-tests/tests/settlement_component_haul.rs` | 2 206 | существующий реальный component/TCP путь и дорогая настройка |
| Loader: `PlayableRealClientLoopScenario.java` | 7 725 | большой графический сценарий |
| Loader: `PlayableRealClientLoopScenarioTest.java` | 6 150 | крупная тестовая поверхность того же сценария |
| Loader: `MinecraftScenarioClient.java` | 4 767 | клиентские действия и ожидания |
| `tools/harness/backends/driver.py` | 4 078 | клиентский driver, кандидат на domain decomposition |
| `tools/harness/backends/wasm_inventory_storage.py` | 2 603 | уже существующие integration сценарии |
| `tools/harness/backends/regression.sh` | 2 017 | orchestration, которую нельзя дублировать новым runner |

Java-файлы находятся в `java-agent/src/{main,test}/java/dev/solaris/agent/javaagent/`.

### 2.2 Исходный план неверно считает несколько вещей отсутствующими

1. Уже существуют 14 workspace crates: `mc-domain`, `mc-data`, `mc-world`,
   `mc-entity`, `mc-physics`, `mc-worldgen`, `mc-nbt`, `mc-protocol`, `mc-script`,
   `mc-plugin-host`, `mc-net`, `mc-server`, `mc-test-harness`, `xtask`.
   Добавление `mc-inventory`/`mc-simulation` само по себе ничего не исправляет.
2. [Архитектурный контракт](ARCHITECTURE.md) уже требует отделять authority от
   адаптеров. [ADR 0006](decisions/0006-mc-net-module-boundaries.md) определяет
   `mc-net` как modular monolith; целевая независимая simulation boundary ещё не
   означает, что она уже вынесена в crate.
3. `python3 -m tools.harness --help` и `list` реально выполнены при подготовке:
   доступны `list`, `run`, `client`, `mcp` и **25** профилей. Числа 23/24 в отдельных
   документах уже расходятся с executable catalog (исправлено в CP-REFACTOR-000:
   `AGENTS.md` и `docs/AGENT_TOOLING.md` теперь называют 25).
4. Focused Rust loop существует: `run test -- -p ... --lib <filter>`.
   Есть `settlement-cargo`, `standard-pack`, `inventory`, `core-client`,
   `loader-live`, `regression`, `playable`, `replay`, `harness-check`, scale/oracle.
5. Уже есть JSON manifests и typed сценарии Rust/Java/Python. Например,
   [core replay manifest](real-client-regression/manifests/core-replay-seed-81.json)
   связывает manifest, runner, scenario id и evidence.
6. Fault injection и process-crash проверки уже существуют: `StorageFaultPoint`,
   `inject_fault_for_test`, `owned_transfer_recovers_exactly_once_across_a_process_crash`,
   `compound_transaction_recovers_inventory_after_process_crash`. Это точки
   повторного использования; при подготовке они **не запускались**.
7. Production plugins — Wasmtime components/WIT 0.7, а не предполагаемый Luau
   путь из старых разделов. В документах сохранились migration-era описания;
   переписывать runtime обратно нельзя.
8. Карта символов уже есть: CodeGraph и rust-analyzer. Статическое «no covering
   tests found» не доказывает отсутствие теста через dynamic dispatch или runner.
   Нужен явный behavioral catalog, а не ещё один обязательный поисковый сервис.

### 2.3 Что известно о CP-049 — и чего этот план не утверждает

Текущий код уже содержит manual warehouse binding и building certificates.
`SettlementLedger::binding_position` и `warehouse_for_position` возвращают `Option`;
неразрешимый authored binding пропускается при cross-kind lookup, а не блокирует
все последующие bind из-за catalog drift. Это конкретная существующая семантика,
которую нельзя превратить в «строгий Result» под видом переноса файлов.

Локальный handoff `.analysis/cp049-resume-20260926.md` фиксирует незакрытые проверки
и предупреждение о прежнем validation tree. Он является историческим свидетельством,
а не разрешением немедленно продолжить старый `next`. В текущем `MEMORY.md` нет
надёжного короткого CP-049 closeout: верхний старый раздел относится ещё к CP-048.

Продуктовый контракт CP-049 шире наличия DTO и трёх binding tests:
manual building capabilities, реальный доступ/станции, invalidation после разрушения,
repair, unload/restart, foreign owner и объяснение отказа. Этот рефакторинг не
дописывает отсутствующие части и не объявляет их выполненными. На возвращении
будет построена матрица «требование → текущий путь → доказательство → пробел».

## 3. Оценка исходного предложения по пунктам

| Исходная часть | Решение | Доработка |
|---|---|---|
| Фаза 0: feature freeze | Оставить | Запрет расширения, не откат существующих CP; обычную потерю предметов/сохранений нельзя игнорировать |
| 1.1 Runtime map | Оставить | Рисовать actual и target раздельно; plugins не стоят после storage в последовательном pipeline |
| 1.2 State ownership | Усилить | Развести semantic owner, native writer, persistence, projection; «warehouse = transaction» неверно |
| 1.3 Mutation map | Оставить | Указать commit/durability/publication, refusal и recovery, не только счастливую стрелку |
| Фаза 2: список crates | Не принимать буквально | Сначала уменьшить связность внутри существующих crates; extraction только с доказанной независимой границей |
| Фаза 3: 500–700 строк | Превратить в ориентир | Размер — сигнал для review, не повод создать 30 forwarding-модулей или разорвать invariant |
| Фаза 4: bounded contexts | Оставить смысл | Доменные модули в текущих репозиториях, не новый общий `domains/` поверх них |
| Фаза 5: unit/integration/simulation/client/performance | Сделать логической классификацией | Не переносить все Cargo/Gradle/Python tests в один корневой `tests/` |
| 5.1 YAML golden scenarios | Заменить | Каталог и дешёвый запуск существующих executable scenarios; без нового языка действий |
| 5.2 failure injection | Сузить и переиспользовать | Инвентаризация existing hooks, конечная матрица реальных crash windows; не chaos platform |
| Фаза 6: пять agent docs | Сократить | Один короткий вход, одна таблица задач; ссылки на существующие router/tooling/ADR вместо пяти копий |
| Фаза 7: harness explain | Оставить | Один каталог для explain/run, точный backend и prerequisites, отсутствие side effects при explain |
| Фаза 8: generated intelligence | Ограничить | Crate graph, scoped contract/scenario index, source freshness; не dump каждого `pub fn` |
| Фаза 9: удалить старое | Изменить критерий | Удалять заменённые пути и ложные инструкции; не уничтожать действующие ADR и уникальные receipts по возрасту |
| Фаза 10: вернуться к CP-049+ | Оставить с gate | Сначала проверяем удобство на CP-049 и отделяем его acceptance debt от завершения DX |
| Оценка 2–3 недели и кратный разгон | Не считать доказанной | Переоценка после пилота; у нас пока нет измерения navigation/build/run/review cost |

## 4. Границы и неизменяемые контракты

### 4.1 Заморожено на время программы

- Новые профессии, здания, производственные цепочки, экономика, война, технологии,
  игровые UI и продолжение CP-050–096.
- Расширение CP-049 сверх существующего поведения ради удобного демонстрационного кейса.
- Переписывание scheduler, ECS, storage engine, journals, protocol или plugin runtime.
- Массовое переименование public API, world schema, stable ids и storage keys ради стиля.
- Новые compatibility shims, runtime selectors, «универсальные» buses/services/transactions.
- Производительная оптимизация без отдельного измеренного bottleneck.
- Перенос first-party gameplay обратно в core; обязательные siblings для production build.

Feature freeze снимается на source/tooling handback CP-REFACTOR-100. Ожидание
отложенных до CP-096 graphical gates не удерживает CP-049–096 в заморозке:
иначе очередь никогда не достигнет checkpoint, после которого разрешена приёмка.
Это не отменяет сами gates и не разрешает назвать клиентское поведение проверенным.

Разрешены переносы и сужение interfaces, удаление реально заменённых paths,
characterization tests для незафиксированного важного поведения, организация fixtures,
навигация, diagnostics и развитие **существующего** harness.

Если обнаружен воспроизводимый баг обычной игры/потеря данных, он оформляется отдельным
bug slice с воспроизведением. Сначала сохранить механический diff; не смешивать
изменение семантики с file move. Косметические и гипотетические редкие края не
перехватывают очередь. Нельзя молча закреплять известную ошибку как новый invariant.

### 4.2 Что должно остаться тем же

- Minecraft 26.1.2 wire и vanilla world-format contract; Mojang bytes не попадают в Git.
- Один owner каждой изменяемой сущности; session/UI/guest snapshots не становятся authority.
- Предметы не появляются и не исчезают из-за переноса; разрешённая частичная выгрузка
  cargo не превращается в новое all-or-nothing правило.
- Отказ до commit не меняет состояние; доставка ответа не определяет факт commit.
- Existing revisions, generation fences, idempotency/receipt rules, lock order,
  journal decision/projection order, durability promises и shutdown semantics.
- Нет allocation/copy/dynamic dispatch/lock/queue в hot path только ради новых модулей.
- Plugin policy остаётся в `solaris-default-plugins`; WIT/DTO/admission — core contract;
  Wasmtime — `mc-plugin-host`; graphical client/scenarios — `solaris-loader`.
- Новая структура тестов не ослабляет assertions, не уменьшает workloads, не меняет
  timeout в PASS и не выдаёт `prepared`/ignored/zero-selected за проверенное поведение.

### 4.3 Что нельзя трогать в механическом срезе без отдельного обоснования

Для каждой карточки перечислить конкретный forbidden write set. По умолчанию:

- wire ids/layouts, WIT shapes, serialization field names, journal/NBT formats;
- generated/sidecar input bytes и production world;
- owner lock acquisition/release, durable append/sync, receipt replay ordering;
- чужие dirty/untracked файлы и локальные инструментальные настройки;
- значения product rules и пакеты deployment.

Если перемещение требует изменить один из этих контрактов, это уже не чистый move:
срез перепланируется и проходит соответствующую behavioral/ADR проверку.

## 5. Целевая форма проекта: ответственность раньше директорий

### 5.1 Runtime map, который нужно получить

```text
Minecraft client
  <-> mc-protocol + mc-net connection/session/transport adapters
          |
          | typed authenticated requests / committed effects
          v
     native admission + current gameplay owners
       |              |                 |
       |              |                 +-> plugin storage/service owner
       |              +-> regional entity ECS owners
       +-> player/world/container transaction authority
                         |
                         +-> world storage / journals / save I/O

WASM settlement policy -> SDK/WIT -> mc-plugin-host -> mc-script admission
                                                    |
                                                    +-> те же native owners
                                                    <- outcomes / observations

mc-server: composition, startup, lifecycle; не второй gameplay owner.
Solaris Loader: optional presentation/transport; не authority.
```

Это схема обязанностей, не обещание одного потока на каждую коробку. В actual map
нужно подписать текущие модули, очереди и threading boundary. В target map можно
показать независимую simulation boundary, но нельзя переименованием сделать её фактом.

### 5.2 Правильная таблица состояния

Начальная карта ниже основана на ADR и исследованных entrypoints; CP-REFACTOR-010
обязан уточнить реальные writer symbols и legacy exceptions каждого выбранного пути.

| State | Semantic/native authority | Reader/projection | Mutation/durability boundary |
|---|---|---|---|
| Player inventory/cursor | native player aggregate; migrated operations через `SimulationOwner` | connection baseline/menu, plugin observations | inventory prepare/owner commit; сохранение по конкретному operation contract |
| Physical chest contents | native container/world state | player menu, warehouse query | existing shared container/inventory commit, не guest KV |
| Warehouse binding/handle | core `SettlementLedger` в plugin storage domain | guest хранит ссылку/назначение | native bind + durable settlement change; binding не владеет содержимым сундука |
| Resident identity/cargo/orders | native resident records/операции; live entity у regional owner | guest roster/policy, client entity projection | resident operation + соответствующий composite/world receipt |
| Household/job intent/economic policy | settlement component, его persisted registry через host storage | UI и policy callbacks | component commands/CAS; не прямое изменение world/entity/inventory |
| Construction progress/certificate | native ledger и world-fenced операции; guest выбирает план | guest/UI observations | prepare/commit/receipt/recovery конкретного native пути |
| Entity motion/health/lifecycle | regional ECS owner | published snapshots/tracking | regional command/transaction, затем publication |
| Block state/block entity | `mc-world` conditional commit; cross-domain ordering у native coordinator | immutable world snapshots | state/token fence; durability не тождественна dirty publication |
| Session/socket lifecycle | network/session adapter | routing indexes | connect/disconnect/transport; не право самостоятельно менять ECS |
| Test run/processes/receipt | core canonical harness | агент/CI читает result artifacts | один lifecycle owner; engines не запускаются обходным путём |

Не писать «Resident принадлежит плагину» без уточнения какого именно record.
Не писать «Warehouse принадлежит transaction»: transaction — граница изменения,
а не владелец данных. Один writer означает один разрешённый путь authority для
конкретного состояния, а не обязательный один гигантский enum на весь сервер.

### 5.3 Mutation cards

Для каждого приоритетного домена нужны 1–3 коротких маршрута:

1. вход и provenance — packet / admitted plugin operation / native scheduled action;
2. DTO и admission — permissions, bounded input, generation/session identity;
3. observations/planning — что можно считать без изменения authority;
4. owner validation — какие revision/token/stock fences перечитываются;
5. commit point и durable decision — где возможен unknown outcome;
6. projection/publication — что может не дойти, не отменяя commit;
7. restart/replay — кто и из какого record восстанавливает состояние;
8. тесты — exact id/selector и что каждый действительно доказывает.

Для UI click отдельно показать cursor/menu и physical container. Для plugin bind —
policy → WIT → DTO → storage/ledger → result, без выдуманного world write. Для
warehouse transfer — оба endpoint и composite receipt; для construction — material
consumption, block edit, progress и recovery. Не рисовать одну и ту же стрелку для
операций с разной durability.

### 5.4 Crate policy

На обязательном маршруте сначала сохранить текущие crates и улучшить внутренние
модули. Модули обязаны уменьшить видимость (`pub(super)`/`pub(crate)` по нужде),
а не делать всё публичным для обхода compiler errors.

`mc-simulation` может быть последующим **отдельным** extraction-решением только если:

- независимая от TCP native boundary уже работает и используется headlessly;
- её state/commands/effects не зависят обратно от `mc-net::SessionRegistry`;
- нет cycle, новых ownership mirrors, proxy traits ради единственного impl;
- measured build/test/navigation cost оправдывает crate boundary;
- все callers и tests можно мигрировать целиком без permanent re-export;
- изменение owning ADR и cross-repo contract входит в тот же проверяемый cutover.

`mc-inventory` не создавать по списку из исходного предложения. `mc-domain` не
превращать в свалку DTO, services и protocol типов. Рефакторинг считается полезным
и без нового crate; выделение crate не является скрытым обязательным условием конца.

## 6. Документация без второго источника истины

### 6.1 Роли документов

| Документ/артефакт | Единственная ответственность |
|---|---|
| `docs/MEMORY.md` | active route, текущий outcome, blockers, exact next, ссылки на evidence |
| Этот план | порядок конечной программы, scope и acceptance; не текущая runtime truth |
| `docs/ARCHITECTURE.md` и owning ADR | нормативные границы и обоснования решений |
| `docs/architecture/runtime.md` | короткая карта actual runtime с пометками ещё не мигрировавшего |
| `docs/architecture/state-ownership.md` | state/owner/read/write/persistence/projection index |
| `docs/architecture/mutations.md` | несколько сквозных mutation cards и ссылки на домены |
| Доменные README рядом с кодом | local contract, allowed/forbidden edits, tests, limitations |
| `docs/AGENT_ROUTES.md` | router к owning документу; не дублирует объяснения |
| `docs/AGENT_TOOLING.md` | executable commands, prerequisites, receipt/error diagnosis |
| `docs/agent/start-here.md` | короткий пользовательский вход в эти источники |
| `docs/agent/common-tasks.md` | task → owner → small read/write set → проверка |
| `docs/generated/` | только воспроизводимая derived navigation, не источник authority |
| `docs/memory/`, evidence, ignored receipts | история и доказательства, не startup reading |

Новые документы создаются при исполнении соответствующей фазы, не все заранее.
Отдельные `module-map.md`, `debugging.md`, `testing.md` не нужны, если они просто
перепишут таблицы выше. Их функции уже распределены между README, maps и tooling.

### 6.2 Короткий startup contract

Целевой вход: applicable repo rules → не более примерно 150 строк живого курсора →
одна карточка задачи/домена → owning contract по затронутому вопросу. Старые логи,
Downloads-plan, release ledgers и репозиторный dump не входят в обязательный пакет.

`MEMORY.md` — не append-only журнал. В нём один актуальный `next`; прошлые `next`
переносятся в историю без потери ссылок и без переобъявления прошлых PASS текущими.
Полный этот план также не перечитывается перед каждой маленькой правкой: курсор
ссылается на текущий CP-REFACTOR и его prerequisites.

В начале исполнения перенести нормативно нужный старый roadmap из персонального
`Downloads` в versioned архив/продуктовый документ, сохранив происхождение и CP ids.
Нельзя иметь обязательный private absolute path для нового checkout. Старый backlog
не переписывать как выполненный и не загружать целиком на старте каждой задачи.

### 6.3 Шаблон domain README

```text
Purpose / Non-goals
Current owner: semantic state / native writer / persisted record
Entry points: commands, queries, events; callers only where relevant
Mutation path: validate -> commit -> durable decision -> projection
Invariants: conservation, identity/revision, ordering, boundedness
Dependencies: allowed directions / forbidden reverse dependencies
Touch here: small set of implementation and tests
Do not touch: protocol/schema/journal/other owner without separate decision
Tests: scenario ids + focused command + what is NOT covered
Known limitations: current defects, deferred acceptance, target not yet implemented
```

Ориентир README — 80–150 строк, не ещё одна спецификация на тысячи строк.
Не переписывать все функции и поля: source/IDE показывают их точнее.

## 7. Тестовая инфраструктура: сделать выбор дешёвым, доказательство честным

### 7.1 Сохраняемые уровни, без массового переноса файлов

| Уровень | Где остаётся реализация | Что доказывает |
|---|---|---|
| Pure domain | owning crate, соседний `*_tests.rs` | bounded decisions, invariants, отказ без mutation |
| Owner simulation | native owner tests | настоящий commit, revisions, command ordering |
| Persistence/fault | owning storage/transaction tests | конкретное окно ошибки, reopen/replay, conservation |
| Plugin integration | plugin sibling `sources/integration-tests` + core API tests | real component/host/native path, deployed bytes/source equality |
| Wire/TCP | `mc-test-harness`, существующие native integration tests | wire/session path; **не** graphical acceptance |
| Real client | Loader Java scenario + existing harness profile/manifest | фактическая client-visible игра/проекция, screenshots где нужны |
| Performance | existing scale profiles | конкретная population/build/environment; не заменяет correctness |

Физические test roots Cargo/Gradle сохраняются. `tests/scenarios/`/`tests/faults/`
на верхнем уровне не создаются ради внешнего порядка. Каталог объединяет поиск,
но не объединяет несовместимые execution/evidence semantics.

### 7.2 Один catalog поверх существующих engines

Добавить к canonical harness discoverable scenario catalog. Минимальная
реализация — обычные typed записи в существующем Python catalog либо небольшом
его модуле, не framework и не новый пакет зависимостей.

Каждая запись содержит:

- устойчивый id, человекочитаемую цель, owning repo/domain;
- evidence kind: pure/native/persistence/component/TCP/graphical/performance;
- ссылку на существующий profile/manifest или exact test selector;
- prerequisites: sibling checkout, toolchain, sidecars, component build, client;
- seed/fixture/config там, где они реально участвуют;
- relevant entrypoints/contract/test source;
- краткое обещание результата и явные ограничения доказательства;
- scope для touched-domain выбора и ожидаемый вид receipt.

Assertions остаются в executable test/scenario. Не копировать условие успеха в
JSON, Python и README одновременно. Существующий graphical manifest остаётся
владельцем steps/expected artifacts; каталог ссылается на него. Никаких `eval`,
shell fragments, arbitrary plugin commands и пользовательского DSL в каталоге.

**Планируемый интерфейс, сейчас НЕ существует:**

```sh
python3 -m tools.harness explain warehouse-bind-manual
python3 -m tools.harness run scenario warehouse-bind-manual
python3 -m tools.harness run scenario warehouse-transfer-recovery
```

`run scenario` — новый profile/dispatch существующего harness, не второй launcher.
Он обязан пользоваться тем же bounded scope, lifecycle, artifact directory и
receipt writer. Не запускать `python -m tools.harness` рекурсивно из каждого шага.
Catalog migration удаляет только действительно заменённые registration/launch
copies; существующие профильные команды не объявляются obsolete автоматически.

`explain` не строит guest, не запускает сервер, не скачивает данные и не получает
credentials. Он печатает resolved entrypoint, prerequisites, files, assertions'
source, evidence level и границы. Машиночитаемый вывод нужен тому же catalog,
а не второму «умному» источнику выбора тестов.

### 7.3 Fail-closed focused run

Результат должен содержать **что действительно исполнилось**:

- profile + scenario id + resolved backend/selector;
- selected/executed/ignored test counts, когда engine их предоставляет;
- источник сценария и revisions всех участвующих repos;
- exact command, cwd, relevant config/seed, fixture/component identity;
- exit code, failed assertion/step, log path и уже существующий result contract.

Нулевое совпадение тестов или ignored-only сценарий не может быть PASS. Unknown id,
неизвестный platform, недоступный backend, stale/missing artifact и timeout не
превращаются в fallback/mock или удачную подготовку. Существующие статусы
`passed`, `failed`, `prepared`, `interrupted` сохраняются; missing prerequisite
пишется как причина, без изобретения отдельного competing receipt protocol.

Exact selector после move обновляется в том же срезе. Rust short filter годится
для ручного L0, но обещанный named scenario должен знать, какую проверку он выбрал.
Выбор «затронутые тесты» сначала явный domain mapping. Автоматический impact-based
selector не считается полным без доказанной полноты; L2 не отменяется.

### 7.4 Первые сценарии — из реального кода

Начальные id ниже — **предлагаемые имена каталога**, не заявления о нынешнем CLI:

| Предлагаемый id | Existing evidence seed | Acceptance при catalog migration |
|---|---|---|
| `warehouse-bind-manual` | `manual_warehouse_binds_surveyed_container_and_reuses_handle` | реальная native binding semantics; fixture ограничения явно названы |
| `warehouse-bind-refusals` | `manual_warehouse_refuses_stale_token_outside_claimed_missing_and_cross_kind`, `manual_bind_refuses_position_held_by_authored_binding` | relevant fences/foreign targets; не выдавать за graphical proof |
| `warehouse-reservation-restart` | `warehouse_reservations_hold_live_stock_across_restart_and_reject_overpromise` | stock reservation при reopen, отсутствие overpromise |
| `warehouse-transfer-recovery` | `owned_transfer_recovers_exactly_once_across_a_process_crash` | реальный child process crash, recovery, ровно один перенос |
| `inventory-storage-recovery` | `compound_transaction_recovers_inventory_after_process_crash` | связанный inventory/storage result после crash |
| `settlement-component-cargo` | existing `settlement-cargo`/sibling component tests | реальные component bytes, host/native path, отдельная цена build |
| `inventory-client` | existing `inventory` profile | реальный клиент и container/cursor assertions |
| `core-replay-seed-81` | existing replay manifest/profile | тот же execution и artifacts, не новая урезанная копия |

Не добавлять `miner_returns_home`/`builder_builds_house`, пока соответствующая
семантика и реальный runner не существуют. Не создавать новую профессию ради
красивого YAML. Недостающий behavioral edge добавляется там, где живёт настоящий
owner; seed и fixture являются входом, а не захардкоженным ответом.

### 7.5 Fault matrix вместо chaos framework

Сначала каталогизировать существующие hooks и проверки; недостающие вводить только
для открытых обязательных окон выбранного mutation path.

| Окно | Что нужно наблюдать |
|---|---|
| Refusal до durable decision | ни inventory, ни world/ledger не изменились |
| Append/sync имеет unknown outcome | не сообщать безопасный отказ и не повторять вслепую; существующий fence/recovery |
| Durable decision есть, projection ещё нет | restart восстанавливает один эффект и одну запись результата |
| Projection есть, delivery/ack потерян | duplicate request не списывает/не выдаёт повторно |
| Guest trap до admitted batch | guest не публикует частичный batch; остальные owners не становятся fallback |
| Disconnect/reload во время операции | существующие session/generation fences и порядок completion |

Точки остановки — именованные production boundary/test hooks с event/pipe/channel
подтверждением. Не `sleep(0.1); kill`, не случайный kill по таймеру. Все ожидания
ограничены timeout, но факт успеха — сообщение/состояние, не истёкшее время.
Не добавлять fault knobs в operator config. Сначала inventory/warehouse/construction
conservation; редкие комбинации отказов не блокируют всю DX-программу без риска
обычной игре или сохранению.

### 7.6 Что действительно сокращает тестовый цикл

1. Сначала использовать уже существующий selector, не запускать workspace после
   каждого метода.
2. Разделить цену build, fixture preparation, execution и client boot в evidence.
3. Измерить три различимых режима: clean/cold baseline, warm unchanged run,
   warm run после типичного локального изменения. Это разные measurement identities;
   не гонять один и тот же успешный gate для успокоения.
4. Разделить огромные suite по доменам и убрать setup, не нужный выбранному контракту.
5. Не создавать server/world/component для pure decision test; не заменять реальный
   owner mock'ом там, где проверяется commit или restart.
6. Вынести immutable fixture data отдельно от per-test mutable world. Независимые
   tmp roots, ports, plugin storage; никаких shared writable global fixtures.
7. Устранить дублирующиеся integration paths только после сравнения assertions,
   prerequisite scope и failure windows. Похожее имя не означает одинаковую проверку.
8. Только после измерения решать compile-unit extraction. Разнесение `tests.rs`
   по модулям **само по себе не ускоряет rustc**; все они всё ещё могут собираться
   в один `mc-net --lib` binary. Нельзя обещать обратное.
9. Не менять memory/thread caps и не выключать checks для рекламного ускорения.
   Существующий cgroup/resource contract harness остаётся в силе.

## 8. Очередь исполнения

Все пункты ниже — рабочий порядок очереди; исполнение начато 2026-09-26 с CP-REFACTOR-000
(checklist §12 отмечает закрытые фазы). Отдельного разрешения на каждую следующую фазу не
требуется: продолжать по порядку, не выбирая очередь заново.
Порядок фаз обязателен; work slices внутри фазы ограничены write set и не служат
поводом останавливать всю исполняемую программу после каждого file move.
Отметки checklist отражают source/tooling outcomes и разрешённые проверки;
deferred graphical obligations ведутся отдельно по правилу CP-REFACTOR-100.

### CP-REFACTOR-000 — заморозка, надёжный baseline, один короткий курсор

**Outcome:** новый агент знает, что делать следующим, и не продолжает CP-050 по
устаревшему `next` из середины MEMORY.

**Работа:**

- Зафиксировать HEAD/dirty scope всех трёх repos и refs deployed components, не
  стирать локальную работу и не переиспользовать неподтверждённый PASS старого дерева.
- Перенести историю из `MEMORY.md` в существующее `docs/memory/` с сохранением evidence
  links. В курсоре оставить freeze, current phase, CP-049 debt, blockers и один next.
- Согласовать `AGENT_ROUTES`, architecture header, tooling и domain references:
  actual/target/historical не должны выглядеть одновременно действующей инструкцией.
- Устранить private Downloads как единственный источник возобновления backlog;
  сохранить CP-049 acceptance и остальные CP ids без реконструкции по памяти.
- Записать небольшой baseline трех developer tasks: найти bind writer и отказ;
  найти inventory crash proof; найти клиентский crafting assertion/runner.
- Разделить в baseline чтение/поиск, выбранные файлы, compile, run, review churn.
  Не требуется телеметрический сервис: достаточно одного воспроизводимого receipt.

**Границы:** runtime/tests/guest bytes не меняются; CP-049 не закрывается. Старые
receipts не уничтожаются и не становятся startup context.

**Приёмка:** cursor около 150 строк; единственный следующий шаг; существующие
ссылки сохранены; другой агент по cursor не выбирает прошлый CP. Docs-only checks,
не Cargo L2 ради Markdown.

### CP-REFACTOR-010 — actual ownership/mutation map и контракт пилота

**Outcome:** ответы «где writer/commit/test/forbidden scope?» получаются по одной
доменной карточке, а не после чтения трёх giant roots.

**Работа:**

- Создать три карты из §6, не дублируя нормативные ADR.
- Первыми описать inventory/container, native settlement ledger, resident entity,
  guest registry и harness lifecycle.
- Указать legacy exceptions вместо красивой, но ложной чистой схемы.
- Различать caller edge от разрешения на mutation; для exports проверить callers
  через CodeGraph/LSP, а не считать количество grep совпадений ownership proof.
- Выписать реальные existing authority/test paths; схлопнуть только доказанные
  дубли в рамках соответствующего последующего cutover.
- Подготовить pilot card: текущие modules/callers, exact baseline tests,
  неизменяемые semantics, allowed write set, список obsolete names после move.

**Приёмка:** bind и transfer имеют разные корректные mutation cards; resident
policy/ECS/storage не перепутаны; у каждой pilot проверки указано, native это или
component/client. Нет общесистемного запрета «неизвестный owner — додумай сам»:
неясность является локальным discovery prerequisite, который надо разрешить.

### CP-REFACTOR-020 — дешёвый scenario entrypoint и диагностируемый harness

**Outcome:** `explain` отвечает, что и зачем запускать; named run реально исполняет
выбранную существующую проверку и оставляет canonical receipt.

**Owned scope:** `tools/harness` catalog/CLI/receipt integration, focused harness tests,
`AGENT_TOOLING`; sibling test ownership не переносится в core.

**Работа:**

- Реализовать §7.2–7.4 для пилотных ids, сохранив старые execution engines.
- Зафиксировать evidence kinds и prerequisites; не делать graphical fallback на TCP.
- Защитить named selectors от zero-selected/ignored-only false green.
- Печатать failed stage/assertion и путь лога без необходимости читать весь output.
- Сохранить единственную реализацию process ownership, deadlines и cleanup.
- Добавить только harness behavior tests: unknown scenario, отказ prerequisites,
  failed child, interruption cleanup, подготовка не PASS, selector не выбрал test.
  Не тестировать текст help, копирование config и echo аргументов вместо поведения.
- Измерить focused native run отдельно от component rebuild и client boot.

**Приёмка:** реальный запуск двух native scenarios и одного existing integration
маршрута; failure smoke с контролируемой причиной; существующий `harness-check`;
unchanged profile semantics. `explain` не создаёт run artifacts и не имеет side effects.

### CP-REFACTOR-030 — один сквозной пилот: warehouse binding

**Outcome:** manual/authored/village binding можно менять, прочитав один local
contract и узкий набор implementation/tests, не весь `settlement.rs`.

**Почему этот пилот:** он прямо связан с остановкой CP-049; уже имеет native tests,
реальные DTO/host/SDK callers и ограниченный mutation contract. Это не новый
«дом построен» сценарий с десятком незакрытых gameplay зависимостей.

**Направление (уточнено в CP-REFACTOR-010; см. `docs/architecture/pilot-warehouse-binding.md`):**

```text
crates/mc-net/src/script/storage/settlement.rs    остаётся корнем модуля (`mod warehouse;`),
                                                  ledger authority и shared atomic `apply`
crates/mc-net/src/script/storage/settlement/
  warehouse.rs       source resolution + bind validation + binding operations
  warehouse_tests.rs три manual-bind теста и их приватные helpers
```

Rust запрещает одновременно `settlement.rs` и `settlement/mod.rs` (E0761), поэтому файл
остаётся корнем, а директория держит только дочерние модули (как `world_inventory.rs` +
`world_inventory/`). Ledger и общий fixture не переносятся спекулятивно: новый файл
появляется только под конкретный compile/privacy/fixture blocker, и причина записывается
в карточку. Не заводить второй ledger в `warehouse.rs`. Survey, construction и
certificate не обязаны переезжать в том же механическом diff.

**Последовательность:**

1. Проверить существующие behavioral tests на текущем baseline и назвать gaps.
2. Сохранить contract `Option` lookup, unload/stale precedence, cross-kind binding,
   handle idempotence и foreign-owner fences, не вводя новый глобальный отказ.
3. Перенести coherent binding code с минимальной видимостью; обновить всех callers,
   imports, test selectors и catalog в том же срезе.
4. Удалить старые helpers/re-exports после cutover; не оставлять forwarding compat.
5. Разделить fixtures и assertions только по реальным границам; no new generic
   builder framework вокруг одного теста.
6. Прогнать focused checks и настоящий existing component cargo path. Проверка
   component path дополняет, но не доказывает весь manual-building CP-049.
7. Провести независимое read-only review законченного пилота, включая negative-code.

**Приёмка:** unchanged native binding outcomes и restart semantics; существующий
component consumer не сломан; root меньше по обязанностям, не просто по строкам;
операция имеет named run и documented forbidden scope. После этого сравнить стоимость
developer task с CP-REFACTOR-000 и локально исправить неудачную форму до массового move.

### CP-REFACTOR-040 — settlement policy/native contracts без giant entrypoints

**Outcome:** политика пакета, native settlement mechanics и API transport различимы
по пути и интерфейсу; типичная policy-правка не требует менять WIT/core.

**Core sub-slices:**

- Продолжить `settlement.rs` по coherent survey, construction lifecycle,
  certificate validation, ledger/recovery sections и sibling tests.
- Разделить `resident_order_tests.rs` по order lifecycle, movement/work, inventory
  и combat/recovery contract; fixtures не должны притягивать все подсистемы.
- Упростить `mc-script/src/lib.rs`: contract types/validation, command/event
  admission, routing/lifecycle — в owning modules. Не менять WIT shape заодно.
- Сохранить небольшой root admission API; не создавать parallel command authority.
- Host/SDK domain modules — conversion boundary; минимум ручных sync points,
  без генератора бизнес-логики и без guest-specific rules в core.

**Plugin sibling sub-slices:**

- Из `sources/solaris-settlements/src/lib.rs` извлечь command parsing/dispatch,
  registry/intent orchestration, warehouse/building policy по фактическим связям.
- Переиспользовать существующие combat/demography/hire/view modules, а не писать
  второй `domains/settlement` рядом с ними.
- У каждого persisted record указать owner, CAS/version contract, consumers и
  restart behavior; lifecycle dispatch не должен дублироваться по модулям.
- Разделить integration setup и сценарные assertions в sibling suite; harness
  лишь запускает sibling-owned runner.
- Rebuild и проверить source/component identity там, где менялся guest source.

**Приёмка:** typical policy-only task локальна sibling; API-change task явно
перечисляет WIT → DTO → host → SDK → guest → tests. Текущие ids/storage keys и
component behavior сохранены; sibling native tests + `settlement-cargo` и affected
standard-pack integration. Никакой новой профессии/демографии/construction policy.

### CP-REFACTOR-050 — inventory, container и simulation ingress

**Outcome:** packet handlers преобразуют input, owner принимает решение, publication
кодирует outcome. Для рассматриваемого path больше нет двух изменяемых authorities.

**Порядок:**

1. Existing inventory prepare/commit; window-0/cursor/crafting и owned endpoints.
2. Container transactions/observers; physical state отдельно от menu projection.
3. Command dispatch и cross-domain ordering `play/simulation.rs`.
4. Оставшиеся touched ingress handlers в `play.rs` и session orchestration.
5. Соответствующие части гигантских session/simulation test roots.

Не переписывать все cases в новый универсальный command bus. Existing
`play/inventory.rs` уже 632 строки и имеет owner-local смысл: он не является
проблемой только потому, что рядом большой файл. Разрезать только смешанные
ответственности, оставлять coherent kernel вместе.

Если найден дублирующий mutable mirror, сначала доказать consumers и current
fences. Cutover переносит **всех** consumers на owner result и удаляет старую
authority. Это отдельный behavioral-risk срез, не косметический rename.

**Приёмка:** conservation/cursor/stale revision/rejection/reconnect/restart по
затронутым paths; focused native и TCP проверки; действительный `inventory`
real-client gate для клиентского поведения при наличии разрешённой graphical
приёмки. Без неё отметить непроверенное, а не назвать preservation доказанным.

### CP-REFACTOR-060 — остальные приоритетные native hotspots

**Outcome:** крупные active roots имеют одну ответственность; полученный порядок
работы не ограничен только settlements.

**Конечный обязательный список исходных hotspots:**

- `mc-entity/src/regional.rs`: registry/routing, local-owner execution,
  cross-owner transaction, migration/persistence, domain tests — разделить по
  существующим boundaries, сохранив transaction/lock order целиком.
- `mc-net/src/play/chunk_stream.rs`: demand/planning, inflight lifecycle,
  prepared publication, cancellation/eviction и tests — без второй cache authority.
- Остатки giant `play.rs`/simulation/session test roots после CP-REFACTOR-050:
  packet decoding остаётся routing, domain policy у своих modules.
- `crates/xtask/src/main.rs`: CLI dispatch отдельно от существующих structural
  rules и других команд; генерация из следующей фазы не наращивает новый monolith.

На старте этой фазы составить **закрытый** список конкретных sub-slices для этих
четырёх областей по актуальным symbols. Новые worldgen/protocol/data files не
добавляются автоматически из LOC ranking. Табличные/generated файлы могут быть
большими с явной причиной; это не оправдание смешанной authority.

**Приёмка:** affected owner/lifecycle tests, unchanged transactions, L2 на
законченную native волну. Если менялся hot execution/ownership boundary — тот же
существующий relevant workload до/после, не уменьшенный benchmark. Если затронут
chunk/client-visible path — соответствующий реальный клиентский сценарий;
сохранить любые отложенные/blocked gate как таковые.

### CP-REFACTOR-070 — Loader и существующие scenario engines

**Outcome:** обычный сбой клиента локализуется до operation/wait/assertion; для
его исправления не нужно читать Java/Python/shell giant files полностью.

**Loader responsibilities:**

- `bridge-core`: typed transport/MCP contracts и наблюдения.
- `java-agent`: действия реального клиента, pushed waits и scenario assertions.
- `fabric-agent`: существующий development client bootstrap; имя сейчас
  вводит в заблуждение, это NeoForge module, не доказательство Fabric runtime.
- `loader-core`/`loader-platform-common`: общие runtime client contracts.
- `loader-fabric`/`loader-neoforge`/`loader-forge`: тонкие platform adapters.

**Работа:**

- Разделить `MinecraftScenarioClient` на coherent actions/observations/waits,
  не делая action успешным до server/client acknowledgement.
- Разделить `PlayableRealClientLoopScenario` на sequential этапы существующей
  истории. Общий state остаётся явным; независимые helpers не создают повторную
  mutable scenario authority. Полный survival loop не заменяется набором smoke.
- Разнести tests по проверяемым контрактам, убрать tests на wording/source layout;
  сохранить проверки физической выполнимости и правильных условий действия.
- Разделить core `driver.py`, `wasm_inventory_storage.py`, `regression.sh` по
  lifecycle/dispatch/scenario responsibility; не создавать второй launcher в Java.
- Схлопнуть duplicated readiness/cleanup/timeout только там, где scope одинаков.
  Shared lifecycle остаётся в canonical harness, client actions — в Loader.
- Привести scenario id/manifest/entrypoint resolution к одному catalog маршруту;
  сохранить provenance, screenshots/state evidence и scenario-specific assertions.

**Разрешённое закрытие source/tooling:** `harness-check`, affected Java tests
через canonical `java` profile, actual headless harness/process smoke и каталог
точных оставшихся client gates. Это не доказательство Minecraft gameplay.
**Клиентская приёмка:** реальный graphical existing inventory/replay или затронутый
этап плюс сохранённый полный gate; при действующем defer переносится в обязательства
CP-REFACTOR-100. `loader-live --platform` нужен для реально затронутых adapters/shared
Loader surface, не автоматически три платформы на server-only Python helper.

### CP-REFACTOR-080 — воспроизводимая навигация и узкие tripwires

**Outcome:** карты зависимостей и scenario resolution не расходятся с executable
источниками и не требуют личного установленного MCP для использования проекта.

**Планируемая команда, пока не существует:**

```sh
cargo run -p xtask -- architecture
cargo run -p xtask -- architecture --check
```

Alias `cargo xtask` добавлять только если соответствует уже принятой Cargo wiring;
не считать несуществующий alias готовым интерфейсом.

**Генерировать:** crate dependency graph из Cargo metadata; grouped public
WIT/command/event contract index со ссылками на source; scenario/profile index
из executable catalog. Не генерировать все private functions и бессмысленный
полный Rust public API dump, который никто не будет читать.

**Требования:** stable sorting/paths, source fingerprint/revision, no wall-clock
churn, deterministic output, `--check` detects stale artifacts without mutation.
Без sibling checkout core graph всё равно строится; sibling-specific entries
явно unavailable, не подменяются пустым «успехом интеграции». Полный cross-repo
inventory требует всех объявленных repos. Generated artifacts не владеют semantics.

Расширить существующий `code-health` только actual dependency/ownership rules:
forbidden reverse imports, duplicate owner entrypoint, stale scenario references
там, где проверка структурна. AST/module structure вместо grep порядка statements.
LOC warning не fail-only норматив; размер сам по себе не behavioral нарушение.
Нельзя gate'ить test assertion wording или объявлять граф доказательством gameplay.

**Приёмка:** генерация идемпотентна, stale change обнаруживается, удалённый symbol
не остаётся в catalog, fresh core checkout не требует персонального CodeGraph.

### CP-REFACTOR-090 — удаление заменённого и санитария docs

**Outcome:** после переноса не остаётся двух действующих реализаций/инструкций.

**Работа:**

- Удалить obsolete helpers, aliases, re-exports, forwarding APIs и adapters,
  callers которых уже мигрированы. Это делается преимущественно внутри их cutover;
  фаза собирает остатки, а не разрешает держать duplication до самого конца.
- Удалить старые scenario entrypoints только при доказанном replacement coverage;
  обновить CI, harness, sibling commands и documentation ссылки.
- Обновить ADR current/accepted/superseded status и разделить прошлый runtime
  rationale от текущего поведения. Действующий старый ADR не является dead doc.
- Удалить конкурирующие инструкции/копии; уникальные причины и receipts оставить
  в историческом архиве вне startup route. Не массовое удаление по дате.
- Сохранить пользовательские untracked файлы, Mojang ignored inputs и доказательства.
- Не объявлять legacy example package мёртвым только из-за отсутствия production
  deployment: выяснить его fixture/reference consumers.

**Приёмка:** поиск retired symbols/entrypoints на их known scope, ссылочная проверка,
no duplicate mutation/test-run authority, отсутствие accidental public re-export.
Удаление не меняет наблюдаемую functional coverage.

### CP-REFACTOR-100 — доказать удобство и передать очередь обратно

**Outcome:** человек и новый агент решают разрешённые контрольные developer tasks
без archaeology; source/tooling очередь закрыта, CP-049 получает маленькую предметную
карточку. Полная клиентская приёмка может оставаться deferred по решению владельца.

**Контрольные задания без добавления фич:**

1. Показать manual bind owner, способ отказа и named test; назвать запрещённый scope.
2. Найти warehouse/inventory crash window и запустить доказательство conservation.
3. Найти guest policy без изменения WIT/native owner и объяснить её test command.
4. Найти клиентский crafting assertion и точный existing runner. Graphical execution
   и диагностику отказа проверить при разрешённой клиентской приёмке; до неё доказать
   только navigation/selector/prerequisite/receipt path через headless harness smoke.
5. По намеренно устаревшему scenario selector получить ошибку, не zero-test PASS.
6. После перезапуска агентского контекста выбрать единственную следующую задачу,
   не загружая checkpoint history.

**Целевые navigation budgets (цели, не уже измеренные результаты):**

- до пяти минут и не более пяти содержательных переходов к owner/mutation/test;
- startup packet около 300 строк без auto-loaded repo rules и без source bodies;
- typical local domain change: 1 owning implementation module + соседний test +
  точечная contract/catalog правка при необходимости, а не обязательная квота 3 файлов;
- root modules целятся в 500–700 строк; более 1 000 требует явной coherent причины,
  а не бессмысленного дробления. Для giant hotspots — отсутствие смешанных
  ответственностей важнее одинаковой длины;
- focused test command не тащит GUI, sibling build или full workspace без причины;
- runtime/build time публикуется отдельно; ускорение заявляется только по
  сравнимому baseline, а не из уменьшившегося LOC.

**Два уровня закрытия, без циклического blocker:**

1. **Source/tooling handback (`draft`).** Все implementation/navigation outcomes
   CP-REFACTOR выполнены; пройдены разрешённые focused, persistence, component/TCP,
   harness/Java/plugin checks, L2 на финальном core tree и независимое read-only
   review. Пройденные headless developer tasks имеют receipts. Для каждого
   отложенного graphical gate указаны exact profile/scenario/manifest, затронутый
   scope, tree identity, отсутствие либо наличие известного failure и точное
   условие исполнения после CP-096. Тогда feature freeze снимается и CP-049
   возобновляется; ждать отдельной смены решения о graphical defer не нужно.
2. **Полная acceptance рефакторинга.** Остаётся открытой до фактического исполнения
   всех сохранённых graphical obligations и полного клиентского developer task.
   До этого нельзя утверждать сохранность всего client-visible поведения,
   успешную graphical verification, `stabilization` или `release-ready`.

Минимальная очередь отложенной приёмки формируется из затронутого scope:
`inventory` после inventory/cursor/container и client-action moves;
`replay`/точный existing chunk scenario после chunk-stream moves;
`playable` после decomposition полного survival loop;
`loader-live --platform <affected>` только для изменённых Loader adapters/shared
runtime. Existing failed owner scenarios сохраняют `blocked`, остальные —
`deferred_by_owner`, ни один не становится PASS из-за Java/unit/TCP checks.
Конкретные manifests/ids выбираются по реальному срезу, а не выдумываются здесь.

Успешные неизменённые гейты той же validation identity не повторять; после
исправления — affected gate. Этот handback не закрывает незавершённую реализацию,
проваленные headless gates или найденную потерю данных: исключение относится
только к уже отложенной владельцем graphical приёмке.

**Передача CP-049:** карточка `construction / manual building recognition` с
реальными current files, state owner, existing operations, BUILD-05/07 requirements,
проверенными/непроверенными gates и первым недостающим outcome. Не обещать, что
этот outcome обязательно помещается в `planner.rs` и один YAML. Не закрывать
CP-048 runtime gaps или CP-049 repair/invalidation документацией.

## 9. Зависимости и управление исполнением

```text
000 baseline/cursor
  -> 010 actual maps + pilot contract
  -> 020 catalog/focused evidence
  -> 030 warehouse pilot + measurement
  -> 040 settlement/contracts
  -> 050 inventory/simulation
  -> 060 remaining native hotspots
  -> 070 Loader/harness engines
  -> 080 generated maps/tripwires
  -> 090 obsolete-path/doc cleanup
  -> 100 final proof + CP-049 handback
```

Это порядок основной очереди, не требование одновременно менять все указанные
файлы. При отдельном разрешении владельца на параллельную работу disjoint slices
внутри native/plugin/Loader областей могут исполняться совместно после фиксации
интерфейсов. Без такого разрешения — последовательная реализация и один независимый
reviewer на значимый закрытый результат. Не делегировать один giant subsystem с
формулировкой «разберись и сделай красиво».

### 9.1 Карточка каждого implementation slice

```text
Outcome: одно наблюдаемое улучшение
Baseline: repo SHA + scoped dirty fingerprint + known failed gates
Read set: current domain README, owning ADR section, конкретные symbols
Write set: exact files/repos
No-touch: authority/schema/wire/journal и чужая работа
Preserved behavior: explicit invariants/refusal precedence
Steps: move -> callers -> delete replaced path -> focused proof
Verification: exact scenarios + evidence kinds + prerequisites
Completion: surviving runtime path, no dead aliases, docs current
Next: одно действие; blockers не спрятаны в истории
```

Первый агент локализует и делает срез, reviewer получает bounded diff/acceptance,
а не весь родительский transcript. Findings классифицируются: нарушение контракта,
воспроизводимый bug, необязательный improvement. Последний не превращается в
бесконечный refactor/revert круг без решения владельца. Исправление findings не
запускает второго reviewer того же checkpoint.

### 9.2 Что делать при неудаче

- Сначала назвать конкретный испорченный invariant/test/dependency, не «агенту не нравится».
- Не исправлять file move добавлением широкой abstraction, нового global failure
  или mirror. Уменьшить границу среза, сохранив конечный outcome программы.
- Если baseline уже красный — записать exact command/receipt и область; не приписывать
  проблему move и не объявлять всё green. Известный owner failure не перепроверять
  ради отрицания наблюдения; rerun только после relevant fix.
- Revert только собственных изменений и только с сохранённой source boundary;
  не `reset --hard` общего дерева. Commit/revert/push требуют соответствующего разрешения.
- Если обязательный graphical gate отложен владельцем, сохранить его открытым по
  двухуровневому правилу CP-REFACTOR-100; он не блокирует source/tooling handback
  и возврат CP-049. Иная недоступность обязательного разрешённого gate остаётся
  обычным blocker: не расширять owner defer на любые неудобные проверки.

## 10. Validation policy и пределы claims

| Изменение | Минимум на edit loop | Закрытие значимого scope |
|---|---|---|
| Docs/maps только | links/paths/status/factual consistency | read-only review; Cargo/client не нужны |
| Pure module move | affected focused tests, caller/import completeness | L1; L2 на завершённую native волну |
| Authority/mirror cutover | invariant/refusal/recovery before-after | same real runtime path, owning ADR, L2, client если surface затронута |
| Harness/catalog | behavior tests + actual named run/failure smoke | `harness-check`, unchanged execution/receipt contract |
| Guest source | native sibling tests + rebuilt component identity | existing component integration, affected package checks |
| Loader/client actions | affected Java tests | real Minecraft scenario, not mocks/TCP; scope-specific platform gates |
| Hot owner/scheduler path | relevant workload before-after | same workload/build/environment; no new perf claims from correctness |

Сегодня, при подготовке плана, выполнены только source/doc research, tracked-file
measurements и harness CLI `--help`/`list`. Gameplay tests, L2, graphical client,
vanilla oracle, crash probes и performance workloads **не запускались**: runtime
не изменялся. Их названия в плане — future acceptance или найденные source entrypoints.

Действующее owner defer graphical acceptance до CP-096 не отменяется этим текстом.
По умолчанию затронутые graphical gates остаются `deferred_by_owner` до указанной
приёмки; не требовать нового разрешения только ради возможности дойти до CP-096.
При source/tooling handback они передаются вместе с CP-049 и не мешают продолжению
продуктовой очереди. Выполнить их раньше можно только если владелец явно изменит
defer. До фактического исполнения соответствующая client-preservation claim открыта;
известный failed client scenario остаётся `blocked` до нужного rerun.

## 11. Когда остановить расширение самого рефакторинга

Программа конечна: выбранные hotspots из §8, catalog и navigation outcomes, а не
«все файлы проекта меньше 700 строк». Дополнительный domain допустим только если
мешает одному из этих outcomes; иначе он остаётся обычной будущей работой.

Не нужны в этой программе:

- новый язык описания gameplay scenarios;
- отдельный repo/code-search service, vector index или обязательный платный LLM tool;
- generic fixture DI/container, repository pattern и интерфейс на каждый struct;
- автоматическая генерация всей бизнес-логики из WIT/schema;
- массовая crate explosion и новые abstract service layers;
- новый scheduler/transaction coordinator/outbox/journal;
- переразметка всех historical docs и всех tests без потребителя;
- обещание «любая задача = 3 файла, 2 теста». Cross-boundary API изменение честно шире.

Если после пилота стало больше hops, wrappers, duplicated registries или test
startup dependencies, он не достиг цели даже при меньших файлах. Исправить форму
пилота до распространения. Если ускорился только поиск, так и записать; не
продавать это как ускорение компиляции, gameplay или качества модели.

## 12. Исполнительный checklist

- [x] **CP-REFACTOR-000:** freeze/baseline, короткий cursor, перенос historical context. Receipt `.analysis/codex-logs/cprefactor-000/`; independent read-only review `changes` → 2 findings исправлены.
- [x] **CP-REFACTOR-010:** actual runtime/state/mutation maps, pilot contract. Receipt `.analysis/codex-logs/cprefactor-010/`; independent read-only review `changes` → 6 findings исправлены; `codegraph sync .` exit=0.
- [x] **CP-REFACTOR-020:** один scenario catalog, explain/run, fail-closed focused evidence. Receipt `.analysis/codex-logs/cprefactor-020/`; independent read-only review `changes` → 3 findings исправлены (replay identity binding, `prepared` wording, §7.3 evidence fields).
- [x] **CP-REFACTOR-030:** warehouse-binding vertical extraction и сравнение developer task. Receipt `.analysis/codex-logs/cprefactor-030/`; independent read-only review `changes` → 3 findings исправлены; pure move подтверждён token-сравнением; hops 3→2, review churn 10128→786 строк.
- [x] **CP-REFACTOR-040:** native settlement/API и sibling policy decomposition; receipt `.analysis/codex-logs/cprefactor-040/`.
- [x] **CP-REFACTOR-050 (source/tooling):** inventory/container/simulation/ingress boundaries и tests; native/TCP gates + reviewer в `.analysis/codex-logs/cprefactor-050/`; graphical `inventory` failed до сценария на startup `basic-economy` и остаётся blocked.
- [x] **CP-REFACTOR-060 (source/tooling):** regional ownership/handle/coordinator/store и tests, chunk planning/inflight/prepare/publication и tests, xtask CLI/rules разделены; `play`/session/simulation оставлены по owner-доказанному no-cut. Native L2, TCP chunk и reviewer — `.analysis/codex-logs/cprefactor-060/`; graphical `replay` дошёл до Play, но failed (`loaded=0`, `DestinationUnloaded`, client chunk-loading timeout), остаётся blocked.
- [x] **CP-REFACTOR-070 (source/tooling):** Loader client delegates + 14 playable stage owners + split contract tests, harness dispatch split, dead loader-block route removed; receipt `.analysis/codex-logs/cprefactor-070/`; independent read-only review pass 0 findings. Graphical acceptance deferred_by_owner.
- [x] **CP-REFACTOR-080 (source/tooling):** deterministic generated navigation (crate graph from cargo metadata workspace_members, contract index from sorted `*.wit` + `ScriptCommand`/`ScriptEventKind`, scenario index from harness `list`/`explain`) with fingerprint `--check`, narrow structural tripwires (catalog ownership as top-level assignment rule, no name-grep), xtask CLI split; receipt `.analysis/codex-logs/cprefactor-080/`; independent read-only review pass 0 findings. Graphical acceptance deferred_by_owner.
- [x] **CP-REFACTOR-090 (source/tooling):** obsolete-path/doc verification sweep, empty write set by design; zero live retired refs, no duplicate harness/driver paths (player inventory still has the direct script/Loader-grant writers documented in [`runtime.md`](architecture/runtime.md)), historical evidence preserved, colony fixture untouched; receipt `.analysis/codex-logs/cprefactor-090/`; independent read-only review pass 0 findings.
- [x] **CP-REFACTOR-100:** `settlement-cargo` passed 20260928T093217 after authorized component rebuild; `standard-pack` 20260928T120442 and `correctness` 20260928T122923 passed; independent review pass; committed 2026-09-28 (owner authorized commits). Graphical gates run 2026-09-28 on owner request: `core-client`, `inventory`, `replay`, `playable`, `loader-live --platform neoforge` passed after stale component rebuild and Loader fixture/scenario repairs (receipts in [`MEMORY.md`](MEMORY.md)).

Оценка сроков — после CP-REFACTOR-030 по actual read/build/run/review receipts.
«2–3 недели» из исходного предложения не является обязательством: один
`regional.rs` содержит больше 22 тысяч строк и требует сохранения ownership,
а разбор клиентских сценариев имеет независимую стоимость запуска. Приоритет —
реальный выигрыш уже после первых трёх фаз, затем конечное закрытие всей очереди,
не бесконечная архитектурная перестройка.

## 13. Источники и старт исполнителя

**Первый вход:** [живой курсор](MEMORY.md), этот план — текущий CP-REFACTOR,
[архитектурный контракт](ARCHITECTURE.md) и только relevant owning ADR.

- [Agent routes](AGENT_ROUTES.md): выбор маршрута.
- [Harness/tooling](AGENT_TOOLING.md): команды, prerequisites, receipts.
- [ADR 0004](decisions/0004-staged-single-writer-simulation.md): simulation authority/ordering.
- [ADR 0005](decisions/0005-regional-simulation.md): regional entity ownership.
- [ADR 0006](decisions/0006-mc-net-module-boundaries.md): modular-monolith boundaries.
- [ADR 0009](decisions/0009-regional-plugin-boundary.md): plugin/native boundary;
  migration-era абзацы требуют отделения от actual state.
- [Plugin contract](PLUGINS.md): production Wasmtime/WIT/deployment.
- [Plugin repository](../../solaris-default-plugins/README.md) и
  [settlements product contract](../../solaris-default-plugins/SETTLEMENT_OVERHAUL_CONTRACT.md).
- [Loader repository](../../solaris-loader/README.md): modules/runtime/tooling.
- [Definition of Done](DEFINITION_OF_DONE.md) и [graphical QA](QA_AGENT_PROTOCOL.md):
  evidence quality; не читать их целиком вместо конкретного scenario contract.

Локальные исходные CP-049 свидетельства: `.analysis/cp049-resume-20260926.md`;
исходный product roadmap: [`archive/PRODUCT_ROADMAP_WASM.md`](archive/PRODUCT_ROADMAP_WASM.md)
(verbatim-копия файла владельца из `~/Downloads`, sha256 внутри; это provenance/ID catalog,
**не** источник следующего `next`), CP-049. Это provenance подготовки, не постоянный
обязательный startup dependency.

**Один `next` живёт только в курсоре.** Порядок фаз — §8, отметки закрытия — §12; текущую
фазу и следующий шаг читать в [`MEMORY.md`](MEMORY.md). Не начинать с переезда crates или
продолжения gameplay.
