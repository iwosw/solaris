# Archived checkpoint log

Чекпоинт-история Solaris, вынесенная из `docs/MEMORY.md`, чтобы живой курсор оставался
маленьким. Файлы нарезаны по **непрерывным** участкам исходного журнала (в порядке, в котором
они там лежали), поэтому повествование каждого чекпоинта не разъезжается по файлам.

**Это не стартовый контекст** (`AGENTS.md`): состояние проекта читается в
[`../MEMORY.md`](../MEMORY.md), сюда заходят за историей конкретной эпохи или за провенансом
решения.

| Часть | Эпоха | О чём |
| --- | --- | --- |
| [log-01-villages-and-content-source.md](log-01-villages-and-content-source.md) | ≤09-14 | Ранняя разведка деревень, границы фич, источник ванильного контента (кэш + импортёр). |
| [log-02-settlement-c1-and-handover.md](log-02-settlement-c1-and-handover.md) | ≤09-14 | Профили поселений, закрытие ядра, путь записи склада (C1), снапшот передачи. |
| [log-03-gameplay-and-loader.md](log-03-gameplay-and-loader.md) | ≤09-14 | Постройки в рельефе, мобы и блуждание, связка плагин↔C4, бой C4, cutover Loader wire. |
| [log-04-releases-and-handoff-fixes.md](log-04-releases-and-handoff-fixes.md) | ≤09-11 | Релизы v0.0.6, починка тестов, серия handoff-issue (сундуки, вёдра, рыба, двойной сундук). |
| [log-05-performance-and-worldgen.md](log-05-performance-and-worldgen.md) | ≤09-14 | Проверенные оптимизации и верификация мира (реки, биомы, берега, свет). |
| [log-06-checkpoints-09-12-to-09-14.md](log-06-checkpoints-09-12-to-09-14.md) | 09-12…09-14 | tab-list, реки, деревни+редстоун, сигналы shutdown, feature executor A1. |
| [log-07-village-generation.md](log-07-village-generation.md) | 09-14 | Feature executor A2 (деревья), активация ванильной генерации деревень, village solver + decor lane. |
| [log-08-audit-closeout-and-inhabitants.md](log-08-audit-closeout-and-inhabitants.md) | 09-15 | Закрытие аудита ядра и жители деревень из маркеров частей. |
| [log-09-live-state-r0-r1-and-core-warehouse.md](log-09-live-state-r0-r1-and-core-warehouse.md) | 09-15/09-16 | Живое состояние, R0/R1-A/R1-B, core warehouse CP-001…CP-003. |
| [log-10-wasm-plugin-host-p0.md](log-10-wasm-plugin-host-p0.md) | 09-16 | WASM plugin host P0: memory bound, hostile-guest evidence, composition root. |
| [log-11-wasm-audit-p1-p3-p4-p5.md](log-11-wasm-audit-p1-p3-p4-p5.md) | 09-16/09-17 | WASM audit, P1 contract/runtime, P3 storage/inventory/menus, P4, P5 blocked. |
| [log-12-p5-p8-and-component-checkpoints.md](log-12-p5-p8-and-component-checkpoints.md) | 09-17/09-18 | P5 accepted…P8 cutover, CP-001…CP-007 component era, architecture review. |
| [log-13-release-close-and-cp008-014.md](log-13-release-close-and-cp008-014.md) | 09-18/09-19 | v0.0.7 close, CI repair, CP-008…CP-014 agricultural/work checkpoints. |
| [log-14-v0-0-8-release-candidate.md](log-14-v0-0-8-release-candidate.md) | 09-20 | v0.0.8 release candidate. |
| [log-15-deferred-acceptance-source-corrections.md](log-15-deferred-acceptance-source-corrections.md) | 09-23 | CP-014 R1 chain и серия source-audit правок с отложенным client acceptance. |
| [log-16-headless-plugin-checkpoints-cp030-046.md](log-16-headless-plugin-checkpoints-cp030-046.md) | 09-23 | CP-030…CP-046 headless: inventory kernel, WIT handles, durable payment, combat, squads. |
| [log-17-latest-checkpoints-cp047-048.md](log-17-latest-checkpoints-cp047-048.md) | 09-25/09-26 | CP-047 combat recovery и CP-048 birth record layer. |

Каждая часть начинается со списка своих секций — по нему ищется нужный чекпоинт без чтения
файла целиком. Части 09–17 нарезаны из журнала 2026-09-26 (CP-REFACTOR-000); текст внутри
перенесён verbatim, изменены только относительные ссылки под `docs/memory/` и добавлен
заголовок части.

Ссылки внутри частей относительны `docs/memory/`; при переносе текста из `docs/` они
переписаны на `../<файл>` / `../../<файл>` (проверено скриптом, битых нет).

Правило пополнения: новый чекпоинт пишется в `../MEMORY.md` (живой курсор); когда он перестаёт
быть текущим, он переезжает сюда — в конец подходящей по времени части или в новую часть, без
переписывания текста.
