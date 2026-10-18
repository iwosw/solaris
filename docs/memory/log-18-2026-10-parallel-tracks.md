# Archived checkpoint log — part 18

Эпоха: 2026-10-01. Факты параллельных треков и решения владельца; живой курсор
остаётся в `docs/MEMORY.md`. **Не стартовый контекст** (`AGENTS.md`): сюда заходят
за механизмом, границей доказательства или провенансом решения.

## Sections in this part

- Tick budget
- Worldgen
- Core purge + plugin API
- Settlements CP-049+
- Tooling+process

Запись содержит **что / зачем / где / доказательство / статус**. Пути ниже
относительны корню Solaris, если не указано иное; ignored receipts не копируются
в журнал. После Main integration ссылки удалённых worktrees переведены на существующие
main sources/receipts; если raw log не перенесён, cited canonical receipt сохраняет его итог. `passed` относится только к явно названному покрытию, не ко всему треку.

---

## Tick budget

### T01 — Измеренные оптимизации, бюджет ещё не достигнут

- **Что:** быстрый выход CPU pool на существующий потолок при устойчивом `TickTime`, заимствование prepared pathing requests и компактные goal results, owner-fenced refresh только motion facts вместо восстановления полных population DTO. Session lock освобождается на owner read; после возврата проверяется lane-version fence. Изменение движения или более поздняя мутация запрещают публикацию.
- **Зачем:** убрать повторную подготовку O(N), не урезая сцену, pathing/physics semantics, cadence или бюджет. Шесть физических ядер — потолок этого хоста, а не новый executor/startup override.
- **Где:** `crates/mc-entity/src/regional/owner_lane.rs`, `crates/mc-entity/src/runtime.rs`; архитектурная граница — `docs/decisions/0004-staged-single-writer-simulation.md`.
- **Доказательство:** `.analysis/codex-logs/tick-budget/receipt.md`: A/B/C по 1200 измерительных тиков, 50 клиентов / 25 регионов / 100k entities; refresh p95/p99 89.796/111.362 → 20.214/21.331 ms; native publication total 104.764/126.778 → 28.395/29.494 ms. Focused mc-entity 646 passed / 7 ignored, mc-net 2072 passed / 6 ignored. Canonical worktree living-world p50/p95/p99 97.176/129.441/159.911 ms; entity-scale passed.
- **Статус:** интегрировано; canonical living-world **failed** относительно 50/60 ms p95/p99. Main-tree correctness, entity-scale и core-client passed; main living-world 95.9/184.7/229.5 ms failed. Swap был заполнен; это не quiet-host результат и не теоретический hardware floor. Процентили стадий не складываются.

### T02 — Dispatch: measured lock cost, entry-size гипотеза отвергнута

- **Что:** профиль Main: около99 290 regional motions/tick, locked_publish7.9/total9.0ms, dispatchp50 10.650ms. Первоначальная гипотеза причины — rewrite~300B snapshot/30MB table>16MB L3 — привела к compact-motion side table, но отдельный microbench её **не подтвердил как основную причину**.
- **Зачем:** пакетный лимит не убирает full-population memory traffic/lock hold; оптимизация должна сокращать саму публикацию, а не менять packet cadence или `last_sent` для неотправленных пакетов.
- **Где:** `crates/mc-net/src/play/session.rs` (`SessionRegistryInner.published_entity_snapshots`), `crates/mc-entity/src/lib.rs` (`EntityIdHasher`, `EntityIdMap<V>`, `EntityIdSet`).
- **Доказательство:** `.analysis/codex-logs/dispatch/prof-a/report.json` (`tick_profile.entity_dispatch`, p50 10650 µs), `.analysis/codex-logs/dispatch/prof-a/run.log`; причинная локализация и 99 290/7.9/9.0 — receipt Main из постановки 2026-10-01.
- **Статус:** **compact PublishedEntities/PublishedMotion side-table REVERTED**, файл experiment удалён. Kept EntityIdMap<ServerEntitySnapshot>: microbench100k get_mut cold-cache SipHash big6.4ms/small72B5.5ms, multiplicative big1.6/small1.4ms показывает dominant hashing+random probing, не entry size. EntityIds server-assigned, не attacker-controlled; SipHash flooding defense здесь не даёт полезной границы. Evidence `/tmp/pubbench` + correction IRC Main2026-10-01; final runtime latency ещё pending.

### T03 — Living-world benchmark не является villager Brain workload

- **Что:** `seed_load_bench_entities` принудительно задаёт всем мобам `Wander { speed: 0.04, period_ticks: 1 }`; у benchmark villagers нет `VillagerData`, поэтому Brain не исполняется.
- **Зачем:** оптимизация Brain/due-set сама по себе не объясняет цену этого gate; менять seeding ради зелёного бюджета нельзя. Pause после прибытия здесь один тик, поэтому sleeping не обещает значительного выигрыша.
- **Где:** `crates/mc-net/src/play/session/load_bench.rs:74-80` (`seed_load_bench_entities`); villager planning — `crates/mc-entity/src/runtime.rs` (`plan_region_villager_updates`).
- **Доказательство:** прочитанный production seed; `.analysis/codex-logs/tick-goals/astra-advice.md`; отсутствие `VillagerData` и Brain scope — факт владельца 2026-10-01.
- **Статус:** установленная граница benchmark, не исправление поведения.

### T04 — Приоритет консультанта: убирать preparation/re-traversal до SIMD

- **Что:** сначала измерять compute/prep/wait/fallback и удалять per-tick O(N) подготовку; затем дешевле читать геометрию и steady state. Рекомендации: pause early-out до full pathing request, scratch reuse/batched queries, реальный villager due-set с incremental neighbor index, учёт fallback reasons, geometry reuse при точном разрешении каждого тела.
- **Зачем:** дистанционный LOD, изменение RNG consumption, объединение goals+physics (между ними game ops) и изменение seeding нарушают semantics. `CPUQuota=600%` не означает шесть эксклюзивных ядер; wall `worker_exec_us` не является CPU time.
- **Где:** `crates/mc-entity/src/runtime.rs`, `crates/mc-entity/src/regional/owner_lane.rs`, `crates/mc-entity/src/regional/owner_lane/physics.rs`.
- **Доказательство:** `.analysis/codex-logs/tick-goals/astra-advice.md`.
- **Статус:** advice/проверяемые гипотезы, не доказанный выигрыш и не обязательство на все предложенные реализации.

### T05 — Food pickup full-population scan — кандидат, не исправление

- **Что:** pick_up_villager_food_locked выполняет any_entity(type_name == minecraft:item) по всем published entities на каждый call.
- **Зачем:** при ~100k это O(N) даже для вопроса о наличии item; возможный item index/deadline index требует отдельного correctness fence.
- **Где:** `crates/mc-net/src/play/session/villager_population.rs` (`pick_up_villager_food_locked`, Main range328).
- **Доказательство:** IRC Main 2026-10-01, source-localized candidate.
- **Статус:** не changed, не measured fix.

### T06 — TickGoals final: бюджет всё ещё failed, moving population доказана

- **Что:** final F unchanged100k workload/50sessions/1200 samples tick85.682/169.264/189.772ms против cleanA0 99.467/163.862/217.541; final stage medians goals19.284/physics25.885/dispatch14.638ms. Libraries647mc-entity+2073mc-net pass, strict Clippy pass; final exact-source entity-scale pass.
- **Зачем:** no idle/Brain shortcut: sampled ~100k selections/requests/candidates, только0.595% grounded+zero velocity visits, fallback reasons sampled zero, benchmark villager types no VillagerData. Median improvement не isolated causality: differing host competition, p95 worse; percentiles не additive.
- **Где:** `crates/mc-entity/src/runtime.rs` (`VillagerBrainEntities`, `plan_region_villager_updates`), existing canonical load workload.
- **Доказательство:** `.analysis/codex-logs/tick-goals/receipt.md`, `final-owner-summary.json`; living result `.analysis/codex-logs/tick-goals/receipt.md`, entity result `.analysis/codex-logs/tick-goals/receipt.md`.
- **Статус:** final living **failed** unchanged50/60ms p95/p99; entity-scale population/correctness **passed**, не latency acceptance. All outbound pressure/drop/retry/timeouts zero; no scene/cadence/RNG/physics skips.

### T07 — Retained navigation reuse обязательно finite active-domain admitted

- **Что:** progressed grounded/horizontal moving/vertical stationary body follows retained next node без нового bounded-candidate calculation лишь если `PathingProbe::can_reuse_retained_path` explicitly admits finite next-foot active chunk без terrain-collision marker; defaultfalse. New target/exhaustion/stall/stop/airborne probes; full collision physics retained, failed move revokes reuse.
- **Зачем:** intermediate entity-scale выявил реальный population escape, не contention-only latency; missing-domain veto нельзя пропускать. Vanilla PathNavigation follows retained nodes/stuck detection, но это не разрешение changing all AI cadence.
- **Где:** `crates/mc-entity/src/lib.rs` (`resolve_retained_pathing`, `PathingProbe::can_reuse_retained_path`); production consumers OwnerPathingProbe/LoadedChunkPathingProbe, regression `retained_navigation_does_not_steer_out_of_the_loaded_simulation_window`.
- **Доказательство:** тот же tick-goals receipt: intermediate escape → active-window consumer fence → fence-only and final entity-scale pass; local Java bytecode constraint, не complete vanilla AI proof.
- **Статус:** fixed+focused/real population verified. Request count не baseline geometric-probe counter; intermediate~6184/6174 probes не finalF count.

### T08 — Direct owner goals: uninterrupted authority вместо population maps

- **Что:** read-only preflight holds ECS handles, then bounded resolve+commit внутри той же uninterrupted owner operation; removes population-sized requests/results/checkpoint maps/activeID vectors+sets/lookups. Missing job-site/nonlocal goal/snapshot override/cross-region villager rejects whole region **before mutation**; drained inputs asserted, standalone/regional stale fences retained.
- **Зачем:** cut preparation/retraversal, не weaken authority или merge goals+physics (между ними game ops). VillagerBrainEntities maintained insert/restore/typeconvert/remove/reinsert/transfer, real due/gossip behavior unchanged. Paused Wander early-out лишь unchanged position/zerohorizontal/no panic/unexpired original resume_tick.
- **Где:** `crates/mc-entity/src/runtime.rs` (`EntityRuntime::{prepare_owner_goal_tick,run_owner_goal_tick}`, `tick_owned_regions`), `runtime_tests.rs`.
- **Доказательство:** tick-goals receipt + final-owner-summary; final100009.36 mean selected, aggregate wall scan14.07/selection9.90/apply31.85/post1.70ms.
- **Статус:** final focused+population verified; diagnostic apply теперь resolve+commit, old resolve keys deleted, old/new apply не comparable и aggregate wall не CPU/critical-path duration.

### T09 — PhysicsChunkLookup snapshot-local, не authority/decoded shape cache

- **Что:** dense eligible physics pass uses ≤256 immutable loaded/missing chunk refs, только ≥4 queries/slot, lifetime snapshot; sparse/wide/out-of-rectangle keeps hash path. Direct candidate sort uses references/collision flags вместо копий full fences.
- **Зачем:** save exact geometry access preparation без material/fluid/shape approximation; original Arc/current-world/lane/entity fences remain.
- **Где:** `crates/mc-entity/src/regional/tick.rs` (`PhysicsChunkLookup`), `regional/owner_lane/physics.rs` (`tick_owned_region_physics`).
- **Доказательство:** tick-goals receipt, final focused/strictClippy/population gates; temporary counting instrumentation removed, no runtime knobs/counter authority.
- **Статус:** implemented/verified final source; next exact swept-bounds carry proposal cost **не measured**, не promised win.

### T10 — Flat perf: hashing заметная CPU доля, точный A/B ещё нужен

- **Что:** Main400tick living perf -F499: hashing~12.6% processCPU (DefaultHasher::write3.3%, sip0.9%, hash_one×4~6.9%); other tops light4.5%, memmove3.7%, floor2.8%, regional publication2.2%, goal inner2.1%, regional physics2.1%.
- **Зачем:** замена hasher ограничена internal server-assigned EntityIds, не blanket hashing rewrite/security weakening для attacker keys. Flat attribution работает, DWARF call graphs — нет.
- **Где:** `crates/mc-entity/src/lib.rs` (EntityId maps/sets), publication/goal/physics owner symbols T02/T08/T09.
- **Доказательство:** `.analysis/codex-logs/dispatch/perf-a/` (run.log/report.json/perf.data); Main correction+perf IRC. TickGoals integrated main patch `.analysis/codex-logs/tick-goals/tick-goals.patch`.
- **Статус:** measured CPU attribution, не isolated latency win. TickHash hot collections и TickPhys exact physics prep follow-ups в работе; combined canonical gate pending.

### T11 — Physics audit не нашёл trig/per-body heap premise

- **Что:** common regional kernel уже gather collision boxes SmallVec<[WorldAabb;32]>, heap только >32 candidates. Sin/cos/sincos/atan2/hypot отсутствуют в scoped physics/regional/local-apply paths.
- **Зачем:** trig caching и generic «убрать heap каждого body» не source-grounded wins здесь; искать измеренную preparation/kernel цену.
- **Где:** `crates/mc-physics/src/lib.rs` (worker range38,1029–1088), `crates/mc-entity/src/regional/tick.rs` (`step_local`), `regional/owner_lane/physics.rs`, `regional/owner_lane.rs` (`apply_local_physics_if_current`).
- **Доказательство:** scoped grep+read IRC `TickPhys`2026-10-01; no runtime speedup receipt для этой отрицательной находки.
- **Статус:** ruled-out premise в названном scope, не performance fix.

### T12 — 400 measured ticks не означают400-sample percentile window

- **Что:** TickHash A workload.measured_ticks400 при tick_profile stage samples1200/source1800/warmup200; retained profiler histograms не isolated measurement-only400tick medians. Baseline tick66.057/205.772/240.467ms,50clients/25regions/100k.
- **Зачем:** короткий diagnostic run включает premeasurement history; whole-process perf включает setup/worldgen. Hash share5.23% в этом profile ниже Main12.6%, не contradiction/isolation proof.
- **Где:** `tools/harness/backends/living_world_scale.sh`; retained tick_profile report contract.
- **Доказательство:** `.analysis/codex-logs/tick-hash/a/report.json`, IRC `TickHash` exact-stage metadata/flatgroups.
- **Статус:** baseline diagnostic, paired A2/B+final receipt pending; не canonical budget pass и не isolated400tick histograms.

### T13 — TickPhys bracket: exact prep cut modest, cache experiment rejected

- **Что:** exact swept-bounds/current-chunk reuse + monotonic ID-sorted replacement walk: bracket physics p5021.087→20.665ms; flat ownerphysics+closure4.98%→3.32%, floor2.29%→1.79%.160entities×256ticks=40960steps position/velocity f64 bits+ground/collision identical SHA256 `4246756269bb62771ee65b9d774bd098388526348ea9d3d0b7fd00bcf1fe9bf0`.
- **Зачем:** measured low-risk exact preparation reduction, не approximate physics. Initial busy26.940ms baseline не isolated causal win; follow-up collision/block-state cache нельзя declare passed из incomplete population.
- **Где:** `crates/mc-entity/src/regional/tick.rs`, `regional/owner_lane/physics.rs`.
- **Доказательство:** `.analysis/codex-logs/tick-phys/before/`, `bounds/`, `before-repeat/` (report/summary/perf); IRC `TickPhys`, directrelease/perf499Hz unchanged100k, MEASURE_TICKS400 budgets0, exported rolling1200samples.
- **Статус:** diagnostic paired exact-prep evidence, не canonical budget pass. Cache experiment **removed** после full100k activation failure active16641/spawn_dispatches46528, no usable stage report; не host-only explanation.

### T14 — TickHash adjacent A2/B: hash CPU share ниже, shared-host medians нестабильны

- **Что:** exact stagedbaseline A2 vs optimizedB, same100k/50clients/25regions/two owner lanes/400measure200warmup/budgets0: visible hash CPU6.27→4.36% (−30.5%relative; DefaultHasher1.68→1.52,sip0.51→0.40,hash_one4.08→2.44). Goals median32.764→21.026ms, physics40.473→30.121, dispatch6.470→7.795; tick102.891/218.762/278.041→73.864/171.582/199.797ms.
- **Зачем:** hashing cut имеет adjacent CPU attribution, но initial samebaselineA66.057ms демонстрирует большой host variance; no causal latency guarantee и rolling1200histogram не400tick-only.
- **Где:** internal EntityId hot collections трека T02/T10.
- **Доказательство:** `.analysis/codex-logs/tick-hash/a2/report.json`, `b/report.json` и corresponding perf-report.txt; общий `.analysis/codex-logs/tick-hash/hash-summary.json`, IRC `TickHash`.
- **Статус:** both runtime smoke profiles exit0/zero disconnects; unchanged latency budget still missed, final coordinated canonical gate pending.

### T15 — TickTail: single-lane warmup загрязняет slow-tail окно

- **Что:**400ticks1401–1800: первые71 lane0/25regions/100001entities median192.100ms/p95212.250; remaining329sixlanes16k–20k median~84/p95~120. Top5%slow all singlelane; goals CPU64.727/wall65.439ms, physicsCPU97.938/wall99.006, no tick%2/5/10 hot residue.
- **Зачем:** tail здесь compute при adaptive convergence, не scheduler wait. Setup owner_lanes1 despite requested6; redistribute1472 после60sec TickTime operator cooldown. Slow sliding window не чистый six-lane steady state.
- **Где:** `crates/mc-net/src/chunk_pipeline.rs` (487–505 ScaleDown physicalCPU), `crates/mc-net/src/server/runtime_control.rs` (owners coupled pool, Hold+TickTime cooldown).
- **Доказательство:** `/home/kaiserroman/solaris-ticktail/.analysis/codex-logs/tick-tail/traced-a1/analysis.json`, `ticks.json`, `report.json`, `summary.json`, `run.log`; IRC `TickTail`.
- **Статус:** traced attribution; Main approved immediate bounded measuredCPU response + active-entity idle-accounting scope, fix/finalgate pending.
- **Main resume уточнение:** regional tick work невидим adaptive CPU pool, поэтому operator cooldown оставляет startup lane collapse. Fix ownership `runtime_control.rs`+`chunk_pipeline.rs`, не scheduler-wait suppression.

### T16 — TickTail B/A/B/A: owner convergence и noisy residual prefix

- **Что:** same400tick windows rollingp50/p95/p99ms B1=54.764/69.282/90.440,A2=70.767/198.102/244.226,B2=61.356/115.999/149.479,A3=64.434/138.117/195.541. BothB6lanes all400ticks16k–20k/lane; A2 содержит100k singlelane,A3 48k–52k twolanes. Production cut immediate bounded TickTime CPU response on Hold +entity-activity idle accounting, no pinnedthreads/scene/budgetchange.
- **Зачем:** сравнение показывает convergence boundary, не обещает quiet-host latency: hostload B1 8.63→6.03,A2 6.03→7.78,B2 7.78→14.75,A3 14.75→7.52. B2physics CPU-vs-wall runqueue tail10.76ms vsB1~0.71ms.
- **Где:** `crates/mc-net/src/server/runtime_control.rs`, `crates/mc-net/src/chunk_pipeline.rs`; owner response/adaptive accounting.
- **Доказательство:** `/home/kaiserroman/solaris-ticktail/.analysis/codex-logs/tick-tail/owners-b1/analysis.json`, `owners-a2/analysis.json`, `owners-b2/analysis.json`, `owners-a3/analysis.json`; IRC `TickTail`. `/home/kaiserroman/solaris-ticktail/.analysis/codex-logs/tick-tail/publication-residual.json`: B1slow publicationwall22.416 vsmedian2.995ms, acquisition/refresh19.625 vs0.001; actual lockedpublish2.564 vs2.877,0extra movements.
- **Статус:** runtime diagnostic B/A evidence, not canonical50/60ms acceptance. Ранняя интерпретация residual prefix как session mutex contention **superseded T18 direct split proof**: expensive stale-fence repair, не lockwait. B2runqueue variance остаётся отдельно measured.

### T17 — PackedBitArray specialization removed; residual prefix attribution уточнена

- **Что:** optional power-of-two storage specialization removed по Main: lowload A2→B3 physicsmedian19.928→20.501ms,tickp95/p9964.965/85.714→73.020/102.917; median-tick/per-lane tiny22.451→22.284/11.130→11.081ms withinnoise. Original world code/tests byte-restored. Packed-b1 pre-seed49/50clients fail, no bodymeasurement, cause unknown.
- **Зачем:** не ship unproven median cut. Owner-only clean windows both6lanes400ticks repeat top5% dispatch7.1→25–26ms, goals~18steady, physics modest23–26; prefix0.001→19.625/18.292ms while lockedpublish2.5–2.9steady,movementplan2µs,no extra moves. No stable modulus hotresidue, weak5tick skew changes0→4.
- **Где:** `crates/mc-world/src/section.rs` (`PackedBitArray`, restored); existing session publication/owner diagnostics.
- **Доказательство:** `/home/kaiserroman/solaris-ticktail/.analysis/codex-logs/tick-tail/packed-a1/`, `packed-b2/`, `packed-a2/`, `packed-b3/`; `/home/kaiserroman/solaris-ticktail/.analysis/codex-logs/tick-tail/post-fix-tails.json`, IRC `TickTail`.
- **Статус:** rejected experiment, no packed fix shipped. Первоначальный holder/acquisition target был гипотезой по combined prefix, **не proven**; T18 опровергает mutex attribution direct split probe. Static25regions/6lanes imbalance max20k/min16k retained; RNG/cadence/physics approximation не применены.

### T18 — CORRECTION: residual dispatch tail = stale-fence repair, не session lock wait

- **Что:** warning-enabled measured400ticks0 sessionwait>10ms warnings; direct split39/400 stale-fence repairs, all20slowest publications repaired. Slowcohort lockmedian0/fence1/retrylock1µs, repair25995.5µs, lockedpublish3316.5µs stable. Repairs sort100k committed states+synchronous coordinator.refresh_movement_publication+lane MatchMovementPublication; repaircohort median24217µs, normalprefix1µs.
- **Зачем:** previous T16/T17 mutexwait attribution неверна: combinedprefix скрывал repair. Root invalidating mutation **ещё unproven**, no stable tickresidue, repairs cluster late; не выдавать correlation за cause.
- **Где:** `crates/mc-net/src/play/session/entity_simulation.rs` (movement publication refresh), native coordinator/lane MatchMovementPublication fence.
- **Доказательство:** `/home/kaiserroman/solaris-ticktail/.analysis/codex-logs/tick-tail/post-fix-locks/lock-warnings.json`, `publication-repair/proof.json`, `publication-repair-probe.patch`, updated `post-fix-tails.json`; IRC `TickTail`explicit correction.
- **Статус:** direct diagnostic proof supersedes mutex claim; temporary probe removed/source-restored tagFFA0, owner convergence fix unchanged. Canonical entity-scale40khostiles pass **не** living50/60gate; local living still misses budget, invalidating mutation/final fix pending.
- **Corrected cohort receipt:** `/home/kaiserroman/solaris-ticktail/.analysis/codex-logs/tick-tail/report.json`: final44ticks1857–1900 had23repairs, не28; total39/400,all20slowest publications repaired. Owner tail comparison middle40tick samples45–55% vs top20 **total ticks**; directsplit comparison all400 **publications** median vs top20publications. Cohorts/rolling1200profile не смешивать. Initial lane phase — недостаточный startup capacity, не observed midrun idle shrink; active-work shrink regression separately demonstrates latent later risk.

### T19 — TickRepair observes invalidator: postphysics nonlethal DamageIfCurrent

- **Что:** movement fence captures после physicscommit, затем fall-damage DamageIfCurrent PrepareAndCommit+Finalize advances coarse laneversion twice. Nonlethal health/damage metadata меняются безmotion, но stale publication sorts~99.5k states/fullrevalidates.
- **Зачем:** T18 unknowninvalidator теперь trace-observed, не routing/visibility/lock/physics hypothesis. Motion-safe health mutation может сохранять movement fence, **generic snapshot fence обязан stale**.
- **Где:** `/home/kaiserroman/solaris-tickrepair/crates/mc-net/src/play/session/entity_simulation.rs`1427–1428 (`resolve_entity_fall_damage` послеcommit), `/home/kaiserroman/solaris-tickrepair/crates/mc-entity/src/runtime.rs`4647–4724 native transactions.
- **Доказательство:** `/home/kaiserroman/solaris-tickrepair/.analysis/codex-logs/tick-repair/probe-A.log`12219–12222; isolated1501–1900 exactly19Damage mutations/19repair ticks, captured→current+2. Tick1672 lane0/id1001227 version8029→8031,99467states refresh26076µs locked2891µs.
- **Статус:** tracecause observed, owner health-only/kinematics+generic-stale tests authored/pass; full650mc-entity/2077mc-net lib pass. Alternating latency/release proof pending, no final win claimed.

### T20 — Opaque movement-only proof removes health repair, coarse fences stay stale

- **Что:** native-equal before/after damage check+last-movement-relevant revision admits unchangedmotion proof while **all coarse revisions advance**. Generic healthsnapshot stale, final operational/authority/coarse fence unchanged; relevant/rollback/topology/lethal failclosed. ID-ordered repair ordinal retained через ordered-run index merge.
- **Зачем:** исключить~20ms fullpopulation sort/native revalidation для health-only delta, не weaken generic entity freshness или packet tracking rotation.
- **Где:** `/home/kaiserroman/solaris-tickrepair/crates/mc-entity/src/regional/owner_lane.rs`, `handle.rs`; `/home/kaiserroman/solaris-tickrepair/crates/mc-net/src/play/session/entity_owner.rs`, `entity_simulation.rs`.
- **Доказательство:** `/home/kaiserroman/solaris-tickrepair/.analysis/codex-logs/tick-repair/` A1/B1/A2/B2(.json/.summary/.run/.log), each400mature1300–1700/exact100k/50clients/25regions. Rollingtick A1 61.800/79.559/91.554→B1 56.678/64.722/67.970ms; A2 57.063/78.425/86.456→B2 57.540/65.806/68.913. Repairprefix20.9185/19.053→0.028/0.027ms(Bmax0.057/0.037),zero>1ms; slow5%dispatch26.8485/24.7615→6.0885/6.106.52Bdamage traces35+17 kinematicsunchanged,44Brepairedticks no fullrefresh.
- **Статус:** paired diagnostic win/independentfence+packet reviewers correct, nofindings; randomized3seeds×18actualtracker publications compare health/randomposes/orderedruns/budgetrotation. Sharedhostload differs,A2p50worsens0.477ms/non-dispatch confound; **50/60budget still unmet**. Final650entity/2077net libraries+strictlib/testsClippy/releasebuild pass; actualdefaultentity-scale40khusks/60clients/1200ticks1pass/0fail (4390269frames/91150663bytes), **не**100klivingbudget acceptance. Remaining physics~26–27ms largeststage, Main exclusivecanonical gate pending.
- **Final delivered proof:** `/home/kaiserroman/solaris-tickrepair/.analysis/codex-logs/tick-repair/receipt.json`: consuming opaqueproof API no mapclone, nativeatomic tuple Clippy-only grouping, finalfence reviewer correct/conf0.98/nofindings failure/drop safety. Temporarytickprobe byte-exact removed, throwaways removed, sevenfilepatch SHA256 `509a94d349df7a213463a19946ed929bcc8c2437ff326e5e8dd1ea1b2bab53ad`. Final-validation/entityscale logs named in owningreceipt; patch ready Mainintegration, not overallbudgetpass.

### T21 — Physics2 invariant type classification preserves dynamic precedence

- **Что:** old entity_physics_kind repeated invariant stringmatches+Swimmer::for_type twice/body; exact type classification cached on existing EntityTypeState construction, dynamic projectile precedence retained. RegionalChunkMask shared helper ownedphysics/reexport regional, Goals2 reuses same prerequisite.
- **Зачем:** reduce100kbody invariant preparation без different motion/category semantics, no duplicate chunkmask authority.
- **Где:** `/home/kaiserroman/solaris-tickphys2/crates/mc-entity/src/regional/tick.rs` (`RegionalChunkMask`), `regional.rs`; EntityTypeState construction/physicskind owner.
- **Доказательство:** `/home/kaiserroman/solaris-tickphys2/.analysis/codex-logs/tick-phys2/`: old/new40960steps2048000B kinematics equal SHA256 `3eab0e8843f4e53f0d68a90255dfcb7c62ec9c5f4b54dd9a65c6889c17e12bab`;157vanillatypes×16projectile/experience componentpermutations classification/motion equality SHA256 `e4f64005904048e98ddee9455cdd1509b34d736aff459404524449da7122487a`.
- **Статус:** exactdump proof beforelive; invariant/class/chunkmask optimization implemented, live stage/canonical acceptance pending.

### T22 — Mature `other`: player-pose candidate preparation, не lighting/worldgen

- **Что:** в unchanged100k living-world diagnostic400ticks1600–2000 counters full-lightcalls/workspaceconstructors/resets/chunkgeneration/disk/preparation/lightencoding имеют **нулевую delta**. Time-sliced inner34s perf не содержит drainage/landforms/sincos/light; ранний whole-process profile (как T10) включает startup и не доказывает mature lighting contention.
- **Зачем:** не вводить speculative lighting cache на основании setupCPU. В Apose `other`median11.435ms/commands7.003ms;23421playerposes, acceptbody candidates3.726ms/tick/ownerbody2.252ms vs movementvalidation0.0447/loadedHashSetclone0.0027.
- **Где:** `/home/kaiserroman/solaris-tickother/.analysis/codex-logs/tick-other/A1.summary.json`, `A1-mature-perf-report.txt`, `Apose.summary.json`, `Apose-subphases.json` в той же директории.
- **Доказательство:** фактический mature same400tickwindow, unchangedscene/budgets0; receipt counters на1600/2000 равны. Hostload A1~7→9/Apose6.66→6.29, поэтому их median нельзя выдавать за isolated A/B gain.
- **Статус:** diagnostic attribution, **не shipped optimization**. Borrowed immutable canonical-facts-name registry refs вместо repeated per-candidate Identifier parsing лишь under test; final equality/live receipt ожидается.

### T23 — Physics2 immutable-pass raw-state cache: frozen exact proof, live attribution bounded

- **Что:** retained per-immutable-pass rawstate-ID cache ускоряет repeated ChunkSection::get; **не кеширует per-body collision decisions**, full-coordinate key exactchecked, existing snapshotArc/fence остаётся authority. Отличается от T13 rejectedcache: есть fullpopulation/equality evidence.
- **Зачем:** cut exact repeated decoding без material/collision approximation или scene/cadence change. Frozen releasefixture4000bodies (1600villagers+5×480animals), canonical1.2spacing/Wander.04period1, fixed64chunk air/stone/water/slab/stair/fence/powder; **не canonical worldgen scene**.
- **Где:** `/home/kaiserroman/solaris-tickphys2/.analysis/codex-logs/tick-phys2/same-scene-microproof.txt`, `same-scene-microproof.json`, `kinematics-cache.bin` в той же директории.
- **Доказательство:**40alternating uncached/cachedpasses минус4warm→36samples each; sampler+kernel median1614→1426.5µs(−11.62%),p951656→1483,p991701→1494,320k frozensteps bitidentical. Dynamic40960stepfixture SHA256 `3eab0e8843f4e53f0d68a90255dfcb7c62ec9c5f4b54dd9a65c6889c17e12bab` matches original.
- **Caveat:** microtimer **не включает snapshot/cache construction**; live B3/C3/B4/C4 включает allcost, physics31.297→27.730/31.4815→29.555ms, но goals/other тоже stronglyimprove при load6.28→12.12→7.84→9.66→8.02, не causal fullwin. Overall A3/C5physics26.49→29.33worse;A4/C6physics29.75→24.11,tickp95/p9980.60/87.73→70.79/77.77.
- **Статус:** cache retained по controlled frozenproof, **strict50/60gate не passed**, exclusivecanonical/Main acceptance отдельно; shared-host live improvement не equivalent isolatedcache gain.

### T24 — TickOther contact filter accepted: borrowed canonical facts, alias/baby exactness

- **Что:** ordinary living broadphase candidate дважды allocated/reparsed ownedIdentifier; replaced by borrowed immutable existing registry facts through canonicalstatic-namemap. Aliases/invalidnames retain legacyfallback, babygeometry unchanged. Это завершённый measuredcut из T22, не lightingcache.
- **Зачем:** actualaccept_player_pose_locked contact preparation дешевле без changingcandidateIDs/nativeAABB/categories/semantics или extraauthority.
- **Где:** `/home/kaiserroman/solaris-tickother/crates/mc-net/src/play/session/player_pose_authority.rs`, sibling `player_pose_adapter_tests.rs`; receipts `/home/kaiserroman/solaris-tickother/.analysis/codex-logs/tick-other/microproof.json`, Apose/B1/A2/B2.summary.json.
- **Доказательство:**8alternatingpairs×10kposes sameprocess actual180spawned6-kindcandidates/chunk,12500contacts/round, candidate-IDsets identicaleachcall; median30.4547→6.10285µs/pose(−79.9609%).471canonicaltypes×None/adult/baby+10alias/invalidcases agreelegacypath, digestFNV1a64 `2f793c1f502d4b54`; release16pass/no failures. Independent readonly ContactExactReview correct/no findings/confidence0.97 (IRC).
- **Live bounds:** A2→B2same1500–1900window `other`15.185→9.4555ms,tickp50/p95/p9974.022/91.171/102.894→61.825/68.759/73.650ms; firstpair globallyworse/noisy. Hostload A2~9.06→7.52,B2~6.30→7.36, therefore не causal fulltick attribution.
- **Статус:** Main acceptedcontrolled sameprocess stageproof с loadcaveats; **нет50/60ms gateclaim**, exclusivecanonical measurement Main-owned.

### T25 — Physics2 final clean deliverable: bounded cache/copy cuts, canonical gate ещё Main-owned

- **Что:** T21classification и T23raw-state cache поставлены с exactRegionalChunkMask, existingshape/factrefs/maximumheight hoist; sampler borrows capturedquery/result, nativecommitexpectedquery copy retained. Empty-side publication merge возвращает existingnonemptyvector, duplicatechunkrectangle insertion skips onlyalreadyinsertedrectangle, querycount unchanged.
- **Зачем:** убрать invariant/copy/hashprep без changing dynamicprojectile precedence, floatorder/RNG/timing, powder-snow bodydependence, None/state0 presence или currentworld/Arc/nativefences. Cachebounded128KiB8192slots, eligible≥128verticalqueries, freshimmutablepass/fullsignedX/Z+losslessY.
- **Где:** `/home/kaiserroman/solaris-tickphys2/crates/mc-entity/src/regional/owner_lane/physics.rs`, `regional/tick.rs`, `regional/tick_physics_tests.rs`, `runtime.rs`; receipt `/home/kaiserroman/solaris-tickphys2/.analysis/codex-logs/tick-phys2/receipt.json`.
- **Доказательство:** final653mc-entity+2078mc-netdebugpassed(7/6ignored), strictlib/testsClippy pass, cleanreleaseharnessbuild; actualdefaultentityscale40000husks60clients16×2500/1200ticks passesactive40000,4482017frames/777MB. Diagnosticp50/p95/p99147.167/260.161/349.314ms **не strict100k gate**.320kfixed+40960dynamicbit-identical/2512classificationpermutations, independentreadonlyallproductionchangescorrect/no findings.
- **Статус:** scopedexactness/deliverycomplete; Main accepts11.62%controlled samplerkernelproof withconstruction/hostcaveats(T23), owns exclusivecanonical before/after+combinedADR. Releasefull-libs intentionallynotrun; P32releasefailures отдельнаяследственнаяветка, не скрыты debugpass. Temp tickerbyte-restored/threefixturemicrotests+scripts removed, finalexecutableclean.
- **Integration:** `/home/kaiserroman/solaris-tickphys2/.analysis/codex-logs/tick-phys2/tick-phys2.patch`28560B SHA256 `2d092c066c6d7b447620bae8c9fb99736218a1849595137ee24892270bae79c8`; applyPhysicsbeforeGoals2: RegionalChunkMask physicsowns, Mainapprovedone-line `regional.rs` reexport reusedGoals2.

### T26 — Goals2 exact lazy planning/lending fetch: stageproof −31.10%, ECS optional boundary retained

- **Что:** IDordered densevillagerplanvector/dueindices, lazy IDindex/integer-hashed gossipcells только после eligible receiver/cooldown; stablepublicationorder без re-sort. ExactPhysics-owned RegionalChunkMask, sparsecandidate directlookup<½store/densemerge earlyexhaustion, mandatorycomponenttuple/Withfilters, persistent IDordered iter_many_mut/fetch_next execution.
- **Зачем:** убрать unconditional villagerhash/cell construction и repeatedECSfetch initialization без changing gossipreceiverprecedence/tiebreak/cooldown, candidate visitation/RNGdraw order/nativecommit/standalonefailure boundaries.
- **Где:** `crates/mc-entity/src/runtime.rs`, `runtime_tests.rs`; integrated receipts `.analysis/codex-logs/tick-goals2/receipt.json`, `tick-goals2.patch` (origin worktree removed послеdelivery).
- **Доказательство:** finalsame-scene4000entity440ticks/400measured twinruntimes alternatingeach-tick; prepare1818.463→1068.978µs/apply889.630→796.794µs,total2708.093→1865.772(−31.10%).3seeds×640ticks/96entities/430orderedRNGdraws exact allnativestate/output/pathing/gossip/goalstats; standard651entity+2078netpass/strictClippy/cleanreleasebuild. Actual100k/50clients25regions400ticks finalsmokeexit0,budgets0 **не strictgate**.
- **Pitfall/correction:** Bevy18.1 EntityRef.get_components tuple с Option<&AnimalState> fails при **unregistered type**, не equivalent отдельномуoptionalget. Два realnative-localregressions поймали, finalmandatorytuple+separateoptionalget сохраняет standalone nonanimal semantics; tests не ослаблены.
- **Статус:** deliverable ready, receipt+patch есть в main; Main explicitlyaccepts controlled stageproof; sharedhostA/B invalidcausalattribution/readinessfailedround не performanceevidence, no local50/60pass. PhysicsRegionalChunkMask prerequisite **excluded from goalsdiff**, applyPhysicsfirst; patchSHA256 `f00f8975a3a328c2e8e50c962eed84298b227257492f985cbfb50a0351a4d746`. Tempreference/RNG/tickerprobes removed/byteexact, combinedexclusivegate+ADR Mainowned.

### T27 — TickOther finalclean100k/50-client actualscene passed, baseline-refresh scope explicit

- **Что:** T24two-filecontactcut ready: finalclean unchanged100kscene/50clients/25×4000/view6/400matureticks **0disconnects**, active100002(2naturalNPC). Sourceonlyhelper also sharedwithprojectiongeometry, filtering/order/RNG/decisioncadence/nativefences unchanged.
- **Зачем:** controlledmicro gain дополнен fullpop native/networksmoke без speculativelightingalgorithm/harness/scene/seedchange; earlier FinalClean preseed18reset/48sessionsfailure no population/no loggedcause, не выдуманная populationregression.
- **Где:** `/home/kaiserroman/solaris-tickother/.analysis/codex-logs/tick-other/receipt.json`, `FinalCleanConnection.{json,run.json,log}` в том же receiptroot; production2files изT24.
- **Доказательство:** retryonly appends playwarn/serverdebug logging;650entity+2079netdebugpass/strictnetlib-testsClippy/readonlycorrect0findings.12chunk/lightcompletearrayfixture hashes old/newequal, light/ticker/poseadapter temporarysources byteexactrestored/allthrowaways removed. Finalprofile1200samples includes setup, **не400mature-only comparison**.
- **Integration caveat:** MainlandedGoals2/Phys2 sixcorefilesrefreshedbaseline; ownedcontactpatch byteidentical, **ownproofs preceded disjointcore refresh**, Main combined/exclusivevalidation отдельно. Patch `/home/kaiserroman/solaris-tickother/.analysis/codex-logs/tick-other/tick-other.patch` SHA256 `28b977efa2e41c8a50685f09b3bce5807edb9a2db33bbee6197169c4b74fd8fe`.
- **Статус:** scopedexactness/livecorrectness deliveryPASS, **не strict50/60ms gate**, no causal sharedhostfulltickclaim.

### T28 — Main exclusive integrated release gate: entity PASS, living-world budget FAIL

- **Что:** Mainexclusivewindow остановило newCargo/tests/clients/Xvfb/bench workloads соседей (read/design/sourceallowed, in-flight finish); затем exact `WINDOW CLOSED`. Canonicalrelease **entity-scalePASS**, **living-world-scaleFAIL**, unchangedhardp95/p99budgets50/60ms.
- **Зачем:** scopedmicro/T20–T27 proofs не заменяют wholeproduct budget acceptance. Combinedcurrentcode tickp50/p95/p99 **55.037/71.372/80.803ms**,1200samples/source2500/active50sessions, zerooutbounddrop/retry/timeout/shed; bottleneck остался, не закрытый performancegoal.
- **Где:** `.analysis/codex-logs/perf-window-entity-scale.log`, `perf-window-living-world-scale.log`; actual livinglog `.analysis/validation/20261001T204457-living-world-scale-xi_m6loz/living-world-scale.log`, entity artifact `.analysis/validation/20261001T203814-entity-scale-epf1ao5r/`.
- **Доказательство:** actualharness entityexit0/403.4s,livingexit101/138.6s; livinglog measurementcomplete1200 и hardp95panic50000µs. Goals15.953/18.379/19.398,physics23.477/30.610/35.546,dispatch6.578/8.596/24.112ms(p50/p95/p99); Main identifiedphysicsmaintail. Percentiles **не additive**, это singleintegratedgate, не isolated before/after causalcomparison.
- **Статус:** exclusivecanonical outcomeobserved, **strict50/60goal unmet** despite retainedcontrolled stagegains. Windowclosed allows workerscommandsagain `-j3/nice`; дальнейшаяdiagnosticoptimization/acceptance — owner/Main.

### T29 — TickCritical support-lowering correctness: exact +0 clipping, arrow stop remains separate

- **Что:** reproduced lowered-support bug: settled living body оставался **y64 вместо63.9375**. Physics correction использует **exact +0 clipped-displacement bits**, three sibling `physics_tests` regressions; не blanket removal embedded/stop behavior.
- **Ownership fence:** `stop_on_solid` — **arrow-only**. Embedded arrow support change обрабатывают actual `ArrowTickInput.should_fall/current_block_state` owner path, **не living kernel**. Existing `changed_support_starts_falling…` regression сохраняет exact RNG reset semantics; **не удалять arrow kernel short-circuit как living-support fix**.
- **Где:** root `/home/kaiserroman/solaris-tickphys3/`; `crates/mc-physics/src/{lib.rs,physics_tests.rs}` (lib762), `crates/mc-entity/src/projectile_26_1_2/arrow.rs`688–705, `crates/mc-net/src/play/session/projectiles.rs`2199–2204/3107.
- **Доказательство:** `.analysis/codex-logs/tick-critical/{settled-support-before,physics-support-fixed}.log` относительно того же root. Before `settled_living_body_follows_lowered_support_like_the_general_resolver` FAIL (1PASS/1FAIL/88filtered, exact position bits differ); after **all90 mc-physics debug lib tests PASS**. Receipts/source paths verified, no rerun.
- **Статус:** scoped correctness regression/fix, **не release/full-product/client/strict-budget acceptance**; TickCritical performance track отдельно.

### T30 — TickCritical measured roots и bounded cutover: RPC guard, movement journal, same-owner body help

- **Before diagnosis:** lifecycle p99 **wall6.334ms /CPU67µs /lockwait1µs**; top death RPC **14.175ms под SessionRegistry guard**, **не entity scan**. Detailed scene: **38/1200 full movement repairs** над ~99k states и **540 health-only fast refences**; two-phase Prepare/Commit falsely marked movement (worker measurement record).
- **Physics ceiling:** six-owner **SUMCPU p50/p95 91.717/105.182ms** против **maxlaneCPU18.135/22.021ms** (`critical-before-summary.json`). ~3–4.5ms balancing ceiling — **оценка из CPU distribution, не measured final improvement**.
- **Cutover design:** bounded **4 FIFO lifecycle snapshot/CAS outside session guard**; bounded **4096 owner-local revision/ID journal**, **unknown/overflow→full fallback**, global-ID packetordinal merge. Pure **512 body tasks** получают help на **SAME owner executors**, **не новый pool**.
- **Correctness fence:** review нашёл preexisting mounted passenger в initial physics `committed_motion`; new sparse checks обязаны сохранять old full-validator rejection. Added **private attachment certificate**: initial physics only if coordinator vehicle indexes empty; full validator certifies true. Это не разрешение sparse path принять mounted body.
- **Где/доказательство:** root `/home/kaiserroman/solaris-tickphys3/`, `.analysis/codex-logs/tick-critical/{critical-before-summary.json,detailed-six-lanes.log,mounted-passenger-before.log,mounted-passenger-after.log}`. Actual native pipeline test `crates/mc-net/src/play/session/tests/regional_physics_refence_tests.rs::mounted_passenger_readset_retains_full_repair_rejection`: **before0PASS/1FAIL**, after **1PASS/0FAIL/2090filtered**, counts/failure verified; expected refresh returns None for mounted readset.
- **Статус:** measured before/source cutover + scoped native rejection proof. **Final latency pending /не budget PASS**, не graphical/integrated performance acceptance.

## Worldgen

### W01 — W0: procedural base отдельно от durable native overlay

- **Что:** native redb storage держит imported chunks, sparse overlay sections, metadata и applied stamps; чистая procedural base не создаёт таких записей. Реальные lifecycle smoke проходят journal replay, owner purge, DB reopen, Anvil export и signed sparse import.
- **Зачем:** ленивый мир не должен превращать прочитанный procedural terrain в бесконечный durable chunk archive; пользовательские изменения и simulation state должны переживать unload/restart.
- **Где:** `crates/mc-world/src/native.rs`, `crates/mc-simulation/src/persistence/native_overlay.rs`, `crates/mc-worldgen/src/earth.rs`; `docs/decisions/0012-earth-worldplan-and-native-overlay.md`, `docs/evidence/worldgen-w0-audit.md`.
- **Доказательство:** `.analysis/codex-logs/worldgen-w0/receipt.md`: пять настоящих lifecycle scenarios; 32 commits p50/p95/p99 11957/13078/14055 µs, DB до/после cohort 565248 bytes, max RSS 82032 KiB, debug elapsed 0.83 s; touched tests и strict Clippy passed.
- **Статус:** W0 реализован/интегрирован. Backup/compaction/disk-full/client/Earth-size performance не доказаны.

### W02 — W1: immutable identity и отдельные operating bounds

- **Что:** checked plane `WorldDefinition`; identity включает нормализованное определение, algorithm artifact и canonical root hashes, но не cache/scratch budgets и не bounds. `PlayableBounds` имеет свою operating revision. Canonical root — 520 bytes, 25 cells по 16 m, lake floor/spill Y82/Y92, outlet 22, discharge 25 от полного upstream.
- **Зачем:** изменение размеров активной области или RAM budget не должно менять мир/старую base; upstream outside-playable нужен гидрологии. Несовпадающий/corrupt committed artifact нельзя молча пересоздавать.
- **Где:** `crates/mc-worldgen/src/earth/definition.rs`, `crates/mc-worldgen/src/earth/prepared.rs`, `crates/mc-server/src/world_command.rs`; `docs/evidence/worldgen-w1.md`, ADR 0012.
- **Доказательство:** `.analysis/codex-logs/worldgen-w1/receipt.md`: P01 реальные cache 2/16 MiB, P02 journal edit/reopen после expand/shrink, P03 discharge 25 для одного playable outlet, P04 identity mismatch; пять process-kill publication boundaries. 851 passed / 25 ignored; strict Clippy и 11 real CLI invocations passed. Bounds revisions 1→2→3 сохранили identity, inspect сохранил hashes.
- **Статус:** W1 offline scope passed; W2 физический solver/W3 lazy plans/W4 persistence/W5 live cutover и metric/sphere не заявлены реализованными. Earth preflight estimates 64 bytes/cell и 4× scratch — гипотезы, не solver measurement.

### W03 — W1 publication: симметричный lease и durable barriers

- **Что:** prepare использует existing lease, verified staging, file/directory barriers и final manifest. Matching orphan может восстановиться; committed missing/corrupt artifact отказывается открываться. Review исправил offline→legacy lease sharing, startup admission race, durability ancestor directories и повторный flush recovered artifact перед commit.
- **Зачем:** final manifest нельзя публиковать поверх readable-but-unflushed root; lease обязан защищать в обоих порядках acquisition и до завершения source admission.
- **Где:** `crates/mc-worldgen/src/earth/prepared.rs`; owning design и review corrections — `docs/evidence/worldgen-w1.md`, ADR 0012.
- **Доказательство:** `.analysis/codex-logs/worldgen-w1/receipt.md`, раздел Independent review и final CLI/process-kill acceptance.
- **Статус:** исправления прошли final focused gates/smoke; physical power-loss injection и project-wide/client acceptance не заявлены.

### W04 — Vanilla обязан совпадать с vanilla layout, а не быть авторским аналогом

- **Что:** директива владельца 2026-10-01 требует vanilla layout. Native 26.1.2 Xoroshiro/noise/climate, multi-noise selection, density graph/splines/blended noise/interpolation используются terrain/height/spawn/biome/village gates; startup fail-closed без native data. Удалена authored village-prototype/seed-zero reward-ruin policy, оставлены neutral physical blueprints/templates/native plan source; revision 25 запрещает молча смешивать старую analogue routing с новой.
- **Зачем:** близкий визуально seed-zero ландшафт и наградная руина не являются vanilla parity; old/new generator mixing портит происхождение мира.
- **Где:** `docs/VILLAGE_GENERATION.md` (native routing/revision); physical root — `crates/mc-worldgen/src/earth.rs` отдельно и этим slice не менялся.
- **Доказательство:** `.analysis/codex-logs/vanilla-worldgen/receipt.md`: seeds 0/81/497625, 33×33 grid, biome/climate 1089/1089 каждый; height MAE 0.02112/0.10193/0.05601 blocks; village starts 4/4, 1/1, 3/3 без extras/missing. Native comparison exit 0, mc-worldgen 255 passed / 3 ignored, focused fixtures 2 passed, strict Clippy и fresh export-only smoke passed.
- **Статус:** native routing + village-start slice verified; **не** all-structure/assembled-piece parity. Aquifers, полный surface/carver/decor/ore, beardifier и non-village/concentric-ring generation остаются gap. Main-owned graphical check и packaged importer integration ещё не выданы за passed.

### W05 — W1 independent review подтвердил offline scope

- **Что:** reviewer подтвердил manifest-last artifact/directory fsync, committed fail-closed, independent bounds/budget identity, offline lease `shareable=false`, startup lease через final storage admission.
- **Зачем:** W1 pass не распространяется на W2, где ещё исправляется physical solver.
- **Где:** `crates/mc-worldgen/src/earth/prepared.rs` (`PreparedWorld::prepare`), `crates/mc-world/src/storage/world_lease.rs`, `crates/mc-server/src/main.rs`.
- **Доказательство:** IRC `W1Review2` + `.analysis/codex-logs/worldgen-w1/receipt.md`; reviewer не повторял builds/tests.
- **Статус:** W1 independent review pass для offline definition/identity/publication.

### W06 — W2 physical root: upstream до chunks, canonical envelope

- **Что:** `WorldPlan::build`: tectonics + physical macro DEM → два фиксированных climate/runoff/hydrology/erosion passes → final climate+hydrology → validation → W1 publication. `BasinFixture` остаётся отдельным plan. `SOLROOT2` + u64 length + SHA256 + canonical JSON/float_roundtrip; manifest schema2 pin exact payload/root.plan/solver source hashes.
- **Зачем:** playable/cache/scratch не формируют географию; полный upstream существует до materialization.
- **Где:** `/home/kaiserroman/solaris-worldgen/crates/mc-worldgen/src/earth/world_plan.rs` (`WorldPlan::build`), там же `prepared.rs`.
- **Доказательство:** IRC `WorldgenW2`; `/home/kaiserroman/solaris-worldgen/.analysis/codex-logs/worldgen-w2/earth-tests-initial.log` (58 pass, T10 failed), `t10-canonical-correction.log` (exact-ID invariance corrected), `affected-package-tests.log` (initial three-package pass).
- **Статус:** implementation `earth-root-v1` / `w2-f64-canonical-order-v1`; final gates/smoke pending, не live Tellus/vanilla cutover. Admission 8192 B/root-or-climate cell + 1 MiB catalogs, 4× residency: small 4096+256 fits 256 MiB; Earth-area build не измерен, conservative bound выше W0 target.

### W07 — W2 направления, local tectonic regimes и water accounting

- **Что:** actual partial-cell area/center distance; ocean seeds area×depth-max на connected submerged component (4-neighbor), exact IDs сохраняются. `depression.cells` — exclusive marginal cells, full footprints идут через descendants; seasonal storage суммирует inclusive stocks только hierarchy roots. Lake evaporation использует residual PET после actualET и wetted inflow/prior stocks; groundwater export — явный boundary loss, год 365 дней.
- **Зачем:** избежать doublecount lake stocks и cache/bounds-dependent seeds. Geographic east/north при native +Z south требует eolian `east*deltaX - north*deltaZ`. Усреднение local normals закрытого translating contact отменяет motion в ложный Passive; regimes разделены local normal-motion classes/shear-sign runs с seam rotation.
- **Где:** `/home/kaiserroman/solaris-worldgen/crates/mc-worldgen/src/earth/grid.rs`; в том же каталоге `climate.rs::ocean_mask_with_seeds`, `hydrology.rs::{seasonal_storage_m3,solve_with_lake_losses}`, `erosion.rs::erode`, `tectonics.rs::{extract_boundaries,boundary_interaction_context}`.
- **Доказательство:** IRC `WorldgenW2` + `W1Review2`; authored real raster/relief `enclosed_translating_plate_extraction_preserves_opposing_local_regimes_and_relief` и real-sediment `north_south_wind_moves_deposited_fines_in_physical_direction`; final receipt pending.
- **Статус:** fixes source-reviewed; terminal/runtime verdict не заявлен.

### W08 — Resource admission одинаково доступен prepare/inspect/bounds

- **Что:** inspect/bounds получили equivalent `--scratch-mib`/`--native-cache-mib` через общие checked `prepare_options`.
- **Зачем:** root, admitted prepare с >256 MiB, должен оставаться управляемым, без скрытого меньшего default последующих команд.
- **Где:** `/home/kaiserroman/solaris-worldgen/crates/mc-server/src/world_command.rs` (`prepare_options`).
- **Доказательство:** IRC `W1Review2`, source-reviewed correction; final CLI smoke pending.
- **Статус:** implemented/source-reviewed, не runtime pass.

### W09 — Marine atmosphere/runoff/ice используют surface, не seabed

- **Что:** connected ocean atmosphere elevation0 (sea surface), не bathymetricDEM; disconnected dry subsea land retains negative physical lapse elevation. physical_water использует ту же surfaceDEM climate aggregate/remap и actual ocean mask. Marine precipitation directly ocean outflow без terrestrial ET/snow/soil/GW reservoirs; ocean evaporation в climate. Ice calves at marine receiver, marine cells не accumulate grounded ice/source dry aeolian fines, но accept land wind sediment.
- **Зачем:** raw submerged terrain height не air surface и океан не terrestrial water/ice reservoir; иначе climate/water budget invents wrong altitude/stocks.
- **Где:** `/home/kaiserroman/solaris-worldgen/crates/mc-worldgen/src/earth/world_plan.rs` (`physical_water`), `hydrology.rs::runoff`, `erosion.rs::route_ice`.
- **Доказательство:** IRC `W1Review2`, найдено WorldgenW2 actual CLI smoke/source-reviewed; `/home/kaiserroman/solaris-worldgen/.analysis/codex-logs/worldgen-w2/earth-surface-corrections.log`68pass, `affected-crate-clippy-final.log`success.
- **Статус:** corrected+focused gates, no final W2 review/package/CLI verdict yet.

### W10 — W2 final numerical slices verified, full row остаётся changes

- **Что:** real earth-root-v1 offline T/C/H/E pipeline delivered, SOLROOT2 canonicalf64/checksum/length/schema2/source math identity. Final3package905pass/25ignored, Earth68pass, strict3crateClippy pass;16seed64-cell cohort включает i64 extrema/exact roundtrips/traces, P01–P04/partial-area invariants pass.
- **Зачем:** actual numerical code + CLI publisher доказаны без fake tiled basin/live cutover. Completion отдельных numerical slices не означает закрытую full W2 row.
- **Где:** `/home/kaiserroman/solaris-worldgen/docs/evidence/worldgen-w2.md`, owning ADR `/home/kaiserroman/solaris-worldgen/docs/decisions/0012-earth-worldplan-and-native-overlay.md`; physical owners W06–W09.
- **Доказательство:** `/home/kaiserroman/solaris-worldgen/.analysis/codex-logs/worldgen-w2/receipt.md`: freshsmall prepare3.002s/62000KiB, inspect0.809s/51420KiB, root7022272B/nativeDB1056768B, five durable file hashes readonly unchanged; zero native records. Seed42 wholly ocean visualblue honestly, no fabricated shore.4096DAG traces/material/atmospheric/seasonal water checks passed. Raised1024MiB scratch root28125619B prepared16.611s/119864KiB, inspect+bound revision2 succeed; default admission/checkedMiBoverflow refuse.
- **Статус:** final scoped numerical/package/CLI evidence supersedes pending W06/W08/W09; fullrow **changes**. C04 productionmetric mapping unsupported, H11 explicit conserving split **не automatic delta geometry**, detailed corridor/fan/terrace/dune/cirque/moraine/fjord catalogs unclosed. Dryinitial one-year reservoirs **не periodic equilibrium/universal two-pass convergence**. Earth-size not prepared/admission exceedsW0target; SLO/artifactretention/inhabitantsloot/importloss ownerchoices open. Writable bounds changed redb physicalbytes (logicalcounts/rootidentity stable); только readonly inspect byte-preserving.
- **Independent final review W1Review2:** final source+package/scoped68/water-balance/raised-inspect/bounds receipts прочитаны, no actionable defects в **delivered slice**; это не stage-completion/live-client/Earth-scale pass.
- **Main milestone scope:** T01–T10/C01–C03/C05/H01–H11/E01–E03 delivered; C04 deferred до W7 metric-height, automatic delta-geometry inference deferred. Saved integrated receipt `.analysis/codex-logs/worldgen-w2/receipt.md`; patch saved/pending Main apply, не уже integrated/live acceptance.

### W11 — W3: geology/basin/water owner context persisted root

- **Что:** PlanePlan сохраняет region_geology context, canonical basin_owner и seasonal water_body_owner rasters.
- **Зачем:** RegionPlan prepare/sample не должен сканировать tectonic catalogs/upstream chains/ocean-lake footprints; merged lake/disconnected ocean IDs требуют единственного canonical root owner.
- **Где:** `/home/kaiserroman/solaris-worldgen3/crates/mc-worldgen/src/earth/world_plan.rs`, `region_geology.rs`, `region_water.rs`.
- **Доказательство:** IRC `WorldgenW3`; scoped nice-n19/debugsymbols-off/nonincremental cargo test mc-worldgen earth::-j2 passed99/0failed, `artifact://2878`.
- **Статус:** implemented/focused Earth gate, full W3 acceptance/CLI/independent review pending; not live cutover.

### W12 — W3 offline RegionPlan accepted, runtime/metric gates отдельно

- **Что:** GeoNodeId root/level/x/z physicaldatum+nominalquadtree partialcells, maxdetail1m/depth30; node seeds pinned WorldIdentity. Bounded root spatialhalo/ancestors,33×33rawcolumns/132edgepoints4corners, canonicaledge/corner owners, conservative child water/sediment budgets, five geologybands/tenlogical materials, ecology. Raw surface отдельно, protectedcarving once; ready CPU sample no disk/globalroot/sibling/upstream traversal.
- **Зачем:** source/CLI proving local lazy plan не означает W4persist/W5blockruntime/fluid/C04metric. Delta branches inferred sediment/depth/windreworking,three conserving branches supportedfan/receivingcell; othermouths staticestuaries. Compact wetcolumn correction **bed-only blockY residual**, no physicalQ/m³/watersurface increase.
- **Где:** `/home/kaiserroman/solaris-worldgen3/crates/mc-worldgen/src/earth/region.rs`, `region_water.rs`; `/home/kaiserroman/solaris-worldgen3/docs/evidence/worldgen-w3.md`, ADR0012.
- **Доказательство:** `/home/kaiserroman/solaris-worldgen3/.analysis/codex-logs/worldgen-w3/receipt.md`:639pass/10ignored/0fail, strict2cratealltargetsClippy pass, sameindependentreviewerclosedallfindings. RealCLI small4096rootcells9ancestors loadedprepare134142µs/whole1.20sRSS66052KiB; wide64rootcells19ancestors291046µs/whole0.70sRSS57828; cold/warm/1MiB samples/budgets/worldid equal, allfive durablehashes/native0counts unchanged.
- **Статус:** specified offlineW3row **pass**, supersedes W11pending. Derived-regions.redb separate hashed/version/world/node checked indexed discardable FIFO cache; corruptionmiss только markedderived, never native/gameplayDB; smallerquota clears onlyderived. Rootcache≤4MiBrecord, logicalbytes/file/RSS не universal/amplification claim. W0SLO/artifactretention/export/importloss stillopen; W4/W5/W7/C04unchanged.
- **Integration:** patch `/home/kaiserroman/solaris-worldgen3/.analysis/codex-logs/worldgen-w3/worldgen-w3.patch` SHA256 `3de17ea366ab0997bbbe15855002edb8b153c8e954b90d5fbee6f18df43f7da5`; Main reconciles9baselinefmt files. Lockexcluded/separatediff onlyexistingpinnedredb directdependency+W2serde_json list, no versionchanges.

### W13 — W4 backend schema2 durable afterimages/clock, exactENOSPC open (2026-10-02)

- **Что:** global canonicalblock-name+properties dictionary, compressed sparse/dense hysteresis/AllAirreplacement, chunk sidestate absoluteu64deadlines+ownerLastUpdate, globalentity afterimages/tombstones+never-reassigned consumedgeneratedslots. Bounded off-tick storage worker exactimage/Arc/revision/pendingWAL acknowledgements; nativeclock API/schema1 path removed cleanly, schema1 explicitlyrefuse/reprepare, no shim.
- **Зачем:** blockID drift/cachepurge resurrection/duplicategeneratedactor/equalLSNfalseclean prevented. Native deadline99 previously became79 flushedat20; relativeAnvilcodec/syntheticLastUpdate cannot be nativeauthority. Partialphysicaledge chunks valid independently playable bounds.
- **Где:** `/home/kaiserroman/solaris-worldgen4/crates/mc-world/src/native.rs`, `storage/native_flush.rs`; `/home/kaiserroman/solaris-worldgen4/crates/mc-worldgen/src/earth/materialization.rs`; `/home/kaiserroman/solaris-worldgen4/docs/evidence/worldgen-w4.md`, ADR0012.
- **Доказательство:** `/home/kaiserroman/solaris-worldgen4/.analysis/codex-logs/worldgen-w4/receipt.md`:972pass/25ignored3crate/strictClippy, native_overlay5/materialization6 pass; realCLIedit/cachedelete/restart/readonlyhash/missingDBrefusal/66m partialedge32accepted33refused. Realprocesscrash/uncommittedrollback+kernelEFBIGrollback, equalLSN/LastUpdate/partialcoverage regressions RED/GREEN; independentfinalreviewcorrect0.97/no findings.
- **Статус:** assignedbackendcomplete, **exactENOSPC nottested** (private user/mount uid_mapEPERM, sharedfs notexhausted), no W5startup/spawn/content/client/metric/sphere/productionimport/export acceptance. Cleanbasezero records, durableair/consumedslots survive; W0SLO/retention/export/importloss choices open.
- **Measuredboundedprobe:**96commits32chunksincl i32extrema/±100million,103.44/s/median9849/p9512801/max14072µs debug, DB155648B/logical48416B/processwrite_bytes3198976 **не physicaldevice/amplification/Earth/MVCC/backup claim**.
- **Integration:** `/home/kaiserroman/solaris-worldgen4/.analysis/codex-logs/worldgen-w4/worldgen-w4.patch` SHA256 `82dd31389fe5b2e9007db4a179e45dab7ff1426b7d35f262015559a242107397`; Main approved nativeclock simulationbridge/tests/example caller migration, no dependency/Lockchange.

### W14 — W5 candidate blocked: native opt-in, marine seed42 refuse до Play

- **Что:** `/home/kaiserroman/solaris-w5` candidate сохраняет default storage **Anvil**, native только explicit opt-in; default worldgen mode намеренно не меняет. Tellus требует native без old-Anvil fallback, автоматически prepares empty directories; `earth_definition` optional, иначе seed-derived small reference.
- **Зачем:** source/old-world invariants запрещают silent source/mode migration и synthetic spawn plateau. User requested wrap current step, но это **не accepted W5 closure**.
- **Где:** `../solaris-w5/docs/evidence/worldgen-w5.md`; receipts `../solaris-w5/.analysis/codex-logs/worldgen-w5/{build.log,native-focused-tests-post-integration.log,startup-focused-tests.log,runtime-smoke.json,review-blockers.json}`; owning ADR0012 маркирует W5 candidate blocked.
- **Доказательство:** build receipt, native-focused **57 pass**, startup-focused **3 pass**. Actual seed42 reference16km square/4096rootcells — **100% ocean**, elevation −4558.06…−4359.83m; auto-prepare committed, inspect exit0/zero native records, startup **failed_before_play**: bounded indexed root не содержит safe dry spawn. Graphical client и existing old-Anvil world open **не exercised**.
- **Open review-only findings (failure scenarios не reproduced):**
  1. P1 UUID allocator после tombstone/restart восстанавливается только из live snapshots → reused deterministic UUID collision при checkpoint.
  2. P2 shutdown inhabited-time tail для evicted native chunk использует synchronous no-generation lookup вместо admission → tail discarded.
  3. P2 entity writer не считает unchanged historical tombstones, reader считает все retained rows → committed image может превысить restart budget.
  4. P1 optional chest-loot fallback оставляет EarthRuntime без item registry → saved nonempty inventory может не decode.
  5. P1 offline unactivated natural-only base и first live village base различаются под same identity → saved metadata может suppress chest/inhabitant markers.
  6. P2 playable expansion reruns bounded spawn search вместо retained committed spawn → startup contract mismatch.
  7. P2 finite-domain view/light halo становится fatal source error вместо uncovered `None` после imported-source resolution → startup abort.
  8. P1 authored settlement-catalog startup ещё требует `world.generator()`; native storage не имеет ChunkGenerator → site owner consumer не wired.
- **Статус:** **NOT CLOSED / candidate blocked**, восемь unresolved findings в exact receipt с source locations. Ни passing focused tests, ни честный marine spawn refusal не закрывают graphical/native/old-world acceptance.

## Core purge + plugin API

### C01 — Нулевая plugin-domain policy в core, широкий generic API

- **Что:** владелец 2026-10-01 требует удалить plugin-domain code **и tests** из core, предоставить широкий generic plugin API; domain policy/fixtures живут в `../solaris-default-plugins`.
- **Зачем:** отсутствие first-party package не должно ломать core build; runtime предоставляет физическую/generic authority, а не встроенную settlement policy. Cutover не сохраняет obsolete schemas/aliases/shims.
- **Где:** `AGENTS.md:249-259` (independent source repositories), `../solaris-default-plugins/AGENTS.md`; текущая очистка — трек SettlementsCP049.
- **Доказательство:** прямая директива владельца 2026-10-01 в постановке архивариусу; `AGENTS.md` подтверждает repository boundary.
- **Статус:** обязательная граница для текущего purge; полнота очистки/нового API будет записана по receipts workers, не предполагается заранее.

### C02 — Generic NPC: отдельные identity, controls и order fences

- **Что:** immutable persistent ownership/lifecycle, physical control/gear/work и order имеют разные fences. Observations дают actual health/max_health/yaw; controls — canonical remaining work/observed targets; morale/custody/population/housing/POIs принадлежат plugin.
- **Зачем:** rename native resident policy не заменяет purge; plugin решает по actual facts, не fake defaults.
- **Где:** `/home/kaiserroman/solaris-settle/crates/mc-script/src/npc_operations.rs` (`ScriptNpcOperation`), `npc_order_operations.rs` (`ScriptNpcControlSnapshot`); `/home/kaiserroman/solaris-settle/crates/mc-simulation/src/services/npcs.rs` (`NpcLedger`), `npcs/orders.rs` (`DurableNpcOrderRecord`); `/home/kaiserroman/solaris-settle/crates/mc-net/src/script/storage/npcs_runtime.rs`.
- **Доказательство:** IRC `NpcEngineBoundary`, source edit receipts; domain originals — `../solaris-default-plugins/.analysis/codex-logs/cp049/domain-test-handoff`. Journal `.npcs/.npc_orders/.world_structures`, batch `.npc/.npc_order/.world_structure/.npc_health_delta`.
- **Статус:** validation pending lead/Main; `mc_entity::TargetCategory::OwnedResident` intentionally unchanged (forbidden scope), не объявлен удалённым.

### C03 — Native physical worksite, plugin purpose/certificate/binding

- **Что:** `RegisterWorksite`/`CheckWorksite` + `QueryBlocks` дают exact identity/sorted properties/walkability/container/door/bed facts; unloaded явный, не fake air. Physical staged blueprint сохраняет once-only block-item cost+WAL. Product purposes/certificates/warehouse-binding — plugin storage.
- **Зачем:** generic integrity/atomics не должны прятать встроенную product policy.
- **Где:** `/home/kaiserroman/solaris-settle/crates/mc-net/src/script/storage/world_structure_runtime.rs`, `/home/kaiserroman/solaris-settle/crates/mc-simulation/src/services/world_structures.rs`; `../solaris-default-plugins/sources/solaris-settlements/src/lib.rs` (`WarehouseBinding`, `warehouse_endpoint`, `storage_get`, `storage_cas`, `record_settlement_population`, `home_generation_id`).
- **Доказательство:** IRC `SettlementsCP049`/`PluginEngineConsumer`: warehouse-binding-v1 отдельно, targets real Container dimension/xyz, stale CAS clears/reloads; home bed:<dim>:xyz + blake3 generation64hex, atomic claims reject crosshome/handle conflicts и Dead/Released spawn.
- **Статус:** implementation, final validation pending. FINAL parent option A: authored geometry автоматически проверяет generic declared interaction-point live integrity, plugin committed StructureStatus owns completeness; дополнительный asynchronous Worksite registration не нужен. Manual explicit Worksite неизменён. Это supersedes промежуточное сообщение о несовместимости authored craft.
- **Дополнение NativeStructures:** authored interaction_points и explicit worksite используют fresh validators; same-ID damage/repair сохраняется. Canonical vanilla bed interaction anchor HEAD only (raw blocks сохраняют обе halves). Paths: `/home/kaiserroman/solaris-settle/crates/mc-net/src/script/storage/world_structure_runtime/worksite.rs`, `construction.rs`; `/home/kaiserroman/solaris-settle/crates/mc-simulation/src/services/world_structures/worksite.rs`, `survey.rs`, `village_sites.rs`. Источник IRC, runtime pending.

### C04 — Native paid nonlethal health delta, без medicine/death policy shortcut

- **Что:** signed nonlethal `ApplyHealthDelta` atomically выполняет real item debit + HP WAL; plugin задаёт material/count/delta (bread1/delta3000). Native rejects unchanged/full-positive и lethal direct afterimage.
- **Зачем:** HP0 сам по себе не выполняет Despawning/death deadline/rewards; damage/death остаются native combat.
- **Где:** `/home/kaiserroman/solaris-settle/crates/mc-net/src/play/session/script_npc_endpoint.rs` (`SessionRegistry::{plan,prepare,publish,replay}_npc_health_delta`); `/home/kaiserroman/solaris-settle/crates/mc-simulation/src/services/storage/records.rs` (`DurableNpcHealthDeltaIntent`).
- **Доказательство:** IRC `SettlementsCP049`/`NpcEngineBoundary`/`PluginInventoryConsumers`, source edits.
- **Статус:** implemented, final atomic/runtime verification pending.

### C05 — Container receipt fence — exact POST generation

- **Что:** POST native chest_state_id, не PRE generation и не minted footprint revision; shared changed-slot count включает container/player/carried slots. Generic afterimage hash+revision immediately reusable.
- **Зачем:** успешный transfer receipt должен правильно fence следующий physical operation.
- **Где:** `/home/kaiserroman/solaris-settle/crates/mc-net/src/play/owned_inventory.rs` (`container_state_change_count`).
- **Доказательство:** IRC `SettlementsCP049`, source receipt.
- **Статус:** correction implementation, final verification pending.

### C06 — Loader schema2 sole id; protocol3 request_kind exact same key

- **Что:** screens.id — sole lowercase namespaced plugin-owned resource key 1..128 ASCII; redundant kind/closed product enums/aliases удалены. Overlay:boolean — HUD renderer only, no key inference. Wire request_kind сохранён и несёт exact key; native key grants требуют current declared screen + same-owner Views+PresentViews same bundle для request и OpenClientView.
- **Зачем:** запретить undeclared screen/cross-bundle grant, без renderer-derived product layout/modal undeclared action/field/tab fallbacks.
- **Где:** `/home/kaiserroman/solaris-settle/crates/mc-net/src/loader.rs` (`LoaderArtifactScreen`, `declared_screen_kinds`), `play/session/script_client_view_endpoint.rs` (`plugin_grants_view_key`); `/home/kaiserroman/solaris-loader/loader-core/src/main/java/dev/solaris/loader/LoaderViewKey.java`.
- **Доказательство:** согласованные IRC `SettlementsCP049`/`NativePresentationBoundary`/`LoaderGenericViews`/`PluginEngineConsumer`; canonical `solaris-settlements:overview`; authored `open_refuses_undeclared_foreign_and_ungranted_screen_definitions` exercises admitted commands, не fake success.
- **Статус:** agreed final schema/implementation, coordinated Loader/client proof pending. WASM/ZIP не rebuilt данным worker; old ZIP с retired kind fails admission. Existing settlements_client gameplay scenario **не modal-widget acceptance**; product graphical replacement принадлежит Main.
- **Проверенная client declaration:** `../solaris-default-plugins/solaris-settlements/client-src/solaris-client.json` (deployable source, не sources/); уточнение `PluginEngineConsumer`, snapshot DE96.

### C07 — Physical startup не зависит от единственного settlement profile

- **Что:** discover_world_structure_deployment_with_hashes грузит все declared physical catalogs, rejects только duplicate blueprint IDs, stable aggregate revision. into_runtime consumes map без cloned keys/allocations; blueprint(id)/blueprint_count queries. Empty-catalog Runtime+LiveWorldStructureWorld устанавливается также без gameplay plugin/catalog.
- **Зачем:** generic QueryBlocks/surveys/vanilla physical facts не должны исчезать из-за отсутствия first-party product deployment.
- **Где:** `/home/kaiserroman/solaris-settle/crates/mc-net/src/world_structures.rs` (`discover_world_structure_deployment_with_hashes`, `WorldStructureDeployment::into_runtime`), `script/storage/world_structure_runtime/construction_tests.rs`.
- **Доказательство:** IRC `NativeStructures`; authored17 staged lifecycle/material conservation/replay/fence/overlap scenarios на physical Container endpoints, не warehouse policy; domain originals archived в plugin handoff.
- **Статус:** implementation/authored tests, final integration run pending.

### C08 — Exact Loader view key имеет production MCP→client caller

- **Что:** minecraft_request_loader_view {view_key:string≤128 required} → request_loader_view → client-thread executor → MinecraftClientFacade.requestLoaderView → registered Consumer<String> SolarisClientAgent → platform requestView → validated key+activated registry/current origin → existing action payload send. Three platform runtimes register reflectively; java-agent без compile dependency на Loader.
- **Зачем:** generic request не остаётся mock/декларацией; invalid/undeclared/inactive throws и не sends, raw payload bypass отсутствует. Graphical verification не заменяется per-frame observer.
- **Где:** `../solaris-loader/bridge-core/src/main/java/dev/solaris/agent/mcp/ClientMcpTools.java`, `client/ClientCommands.java`, `client/ClientFacade.java`; `../solaris-loader/java-agent/src/main/java/dev/solaris/agent/javaagent/MinecraftClientFacade.java`, `SolarisClientAgent.java`; fabric/neoforge/forge platform Loader*McpRuntime и Solaris*Loader.requestView.
- **Доказательство:** IRC `LoaderGenericViews`, verified source paths; final runtime/client receipt pending.
- **Статус:** implemented, parent-owned screenshots+native exact widget clicks required. loader-platform-common **не standalone Gradle module**: common tests входят sourceSets всех трёх platforms, final commands не вызывают :loader-platform-common:test.

### C09 — Generated marker UUID — lookup key, не invented actor

- **Что:** world_structures читает Chunk.generated_entities/source_entity_identity::generated_entity_uuid (algorithm unchanged). Feature/ListFeatures async query npc_entity_snapshots возвращает **actual** snapshot.uuid; absent source actor omitted, unavailable authority/cardinality mismatch RuntimeUnavailable.
- **Зачем:** template marker не доказывает admitted entity descriptor; no synthetic home/jobsite/meeting facts.
- **Где:** `/home/kaiserroman/solaris-settle/crates/mc-net/src/world_structures.rs` (`village_inhabitants`), `script/storage/world_structure_runtime.rs` (`feature_snapshot`, `list_features`).
- **Доказательство:** IRC `NativeStructures` по requested NpcNativeCallerGlue followup, verified source paths.
- **Статус:** implemented, no midflight validation; final authority/consumer gate pending.

### C10 — Native worldgen harness не проверяет plugin birth/population

- **Что:** removed+archived custom-source/home/population food_share_birth scenario. Preserved village villager wire/restart использует actual vanilla jigsaw CacheTags/VillageDecor/VillagePlanSource, native source identity+negative Age. Authored ruin test replaced native village chest loot→inventory→restart.
- **Зачем:** удалить product acceptance из core, не теряя native generated-actor/vanilla chest storage-wire proof.
- **Где:** `/home/kaiserroman/solaris-settle/crates/mc-test-harness/tests/worldgen.rs` (`generated_village_villager_wire_and_restart_are_stable`, `generated_village_chest_loot_moves_to_inventory_and_survives_restart`).
- **Доказательство:** IRC `NpcNativeCallerGlue` + actual definitions193/672; stale owning evidence doc lists направлены lead для caller migration.
- **Статус:** scenarios authored/migrated, no validation executed; не passed.

### C11 — Component resident fixture original сохранён до generic migration

- **Что:** byte-original core wasm_resident_operations fixture55964bytes/1359lines архивирован до rename/migration wasm_npc_operations; preexisting archive не было.
- **Зачем:** domain acceptance provenance не теряется при clean cutover, но старый product target не сохраняется core alias.
- **Где:** `/home/kaiserroman/solaris-settle/crates/mc-test-harness/tests/wasm_npc_operations.rs`.
- **Доказательство:** `../solaris-default-plugins/.analysis/codex-logs/cp049/domain-test-handoff/crates/mc-test-harness/tests/wasm_resident_operations.rs`, IRC `NativeComponentFixture`.
- **Статус:** original preserved/migration underway, runtime/generic component acceptance pending.

### C12 — Generic on-demand received ItemStack component inspection

- **Что:** authorized minecraft_read_item_components {source:selected|player|container,slot0..32767}; selected forbids slot/native selectedindex, others require slot. Container requires actual visible AbstractContainerScreen owning player.containerMenu, no playerfallback. Actual output item/count/present native componentIDs, containerID/stateID; actual emptyair/count0/components{}.
- **Зачем:** quality/product enums не должны попадать core/Loader; real received components inspected on demand, не tick observer mock. Native TypedDataComponent.encodeValue(JsonOps) registry-aware; CUSTOM_DATA {snbt:actual CustomData.toString()} preserves typed NBT/absence, **не core canonical bytes**. Unregistered/no-codec/codec error fails entire read.
- **Где:** `../solaris-loader/bridge-core/src/main/java/dev/solaris/agent/client/ClientCommands.java`, `ClientFacade.java`, `../solaris-loader/bridge-core/src/main/java/dev/solaris/agent/mcp/ClientMcpTools.java`; `../solaris-loader/java-agent/src/main/java/dev/solaris/agent/javaagent/MinecraftClientFacade.java`, `MinecraftClientObservation.java` (`readItemComponents`); `../solaris-loader/README.md`370–387.
- **Доказательство:** IRC `LoaderGenericViews`, local26.1.2 javap signatures/CustomData→CompoundTag→StringTagVisitor. Main/lead authorized; NativeClient read-only mirrors generic contract.
- **Статус:** implemented/source-grounded only; no build/tests/lint/fmt/GUI under source-only directive, runtime proof pending.

### C13 — Actual native EditBox mutation, не model/packet shortcut

- **Что:** authorized minecraft_set_screen_input требует expected class/title/inputlabel/value, class/title1..256,label1..128,value0..256 (clear допустим), value fits native maxLength. Exactcurrent screen+unique actual native EditBoxlabel+active/visible/editable guards → native focus/setValue responder → verify exactvalue/stillcurrent widget.
- **Зачем:** realwidget interaction не rawmodel/directpacket/fabricated API. Lazy cached reflection guards private maxLength/isEditable fail explicitrefusal if inaccessible; public native value/focus APIs javap-grounded.
- **Где:** `../solaris-loader/bridge-core/src/main/java/dev/solaris/agent/mcp/ClientMcpTools.java` (`screenInputSchema`), `../solaris-loader/bridge-core/src/main/java/dev/solaris/agent/client/ClientCommands.java` (`set_screen_input`), `ClientFacade.java`, `../solaris-loader/java-agent/src/main/java/dev/solaris/agent/javaagent/MinecraftClientFacade.java` (`setScreenInput`); `../solaris-loader/README.md`404–413.
- **Доказательство:** IRC `LoaderGenericViews`, Main/lead authorization +local26.1.2javap. `../solaris-loader/loader-platform-common/src/main/java/dev/solaris/loader/minecraft/LoaderViewScreen.java` теперь hides absent-model declared fields/actions/skips emptysections, WorldMarkerSelect requires matchingliveaction/enabled; old enabled-noop/placeholder fallbacks removed.
- **Статус:** source-only implementation,32widgets bound unchanged; real tests/builds/GUI leadpending, не graphical proof.

### C14 — Observed recent_chat newest-first, command-history отдельно

- **Что:** vanilla ChatComponent.addMessageToQueue(GuiMessage) allMessages.addFirst/removeLast>100; Loader recentChat preserves indices0..min20 без reorder.
- **Зачем:** consumer freshmessage suffix→prefix overlap сначала reverses observedlist к chronological. Outgoing commandhistory recentChat.addLast — другой field, не visiblemessage order.
- **Где:** `../solaris-loader/java-agent/src/main/java/dev/solaris/agent/javaagent/MinecraftClientObservation.java` (`recentChat`);26.1.2ChatComponent mapped bytecode.
- **Доказательство:** IRC `LoaderGenericViews`, javap `artifact://3116:723-738`; PluginEngineConsumer consumer correction coordinated.
- **Статус:** bytecode/source audit, no build/runtime check, not chat scenario acceptance.

### C15 — Persistence cutover: clock-aware native fixture, не ослабление journal/validators

- **Что:** новый population fixture смешивал lifecycle100 с `Into` snapshot conversion, дававшим lifetimeAge0. **Только test** исправлен на existing **`PersistedEntityRecord::from_snapshot_at_lifecycle_clock(snapshot,100)`**; native ages **−24000/−1200/0/6000**, food/courtship semantics сохранены.
- **Где:** root `/home/kaiserroman/solaris-settle/`, **один файл** `crates/mc-simulation/src/persistence_tests.rs`; `native_villager_age_food_and_courtship_survive_restart_without_home` checkpoint near347.
- **Deleted obsolete test:** entire `regional_decision_journal_retains_accepted_tail_after_writer_failure` (old679–714) сравнивал private `journal.pending` vector и ожидал **UNKNOWN для known-rejected pre-send write**. Удалена incidental/private-state authority; actual native **known-failed pickup consumer regression сохранён**.
- **Доказательство:** `.analysis/codex-logs/settle/cp049/native-persistence-final-cutover-fixed/{result.json,run.log}` — `nice -n 19 cargo test -p mc-simulation --lib persistence -j 3 -- --nocapture`, **status=passed/exit0,126 PASS/1 ignored**, receipt/count verified.
- **Статус:** fixture/test cutover proof; **no `persistence.rs`/FileRegionalDecisionJournal/validators/W5 allocation-high-water implementation edits**, diagnostics removed; W5 notified no overlap. **Не W5/GUI/L2/whole-product acceptance**.

## Settlements CP-049+

### S01 — Freeze CP-049–096 снят

- **Что:** директива владельца 2026-10-01 снимает feature freeze CP-049–096.
- **Зачем:** прежний pause после CP-048 не является текущим запретом на следующий slice; снятие freeze не отменяет generic-core/domain-plugin boundary и не означает автоматическую client acceptance.
- **Где:** историческая граница — `docs/memory/log-17-latest-checkpoints-cp047-048.md` (CP-048); repository boundary — `AGENTS.md:249-259`.
- **Доказательство:** прямая директива владельца 2026-10-01 в постановке архивариусу.
- **Статус:** freeze снят; отдельные CP и runtime/client gates фиксируются только по фактическим receipts.

### S02 — Plugin morale сохраняет real BlockedRoute provenance

- **Что:** distinct injury/flank/witnessed-allied-death/officer-loss, physical rally/named officer, no quiet recovery. Exact stable-id Attack/Retreat/Cancel intent persists до issuance, receipts merge CAS; TimerFired settlement-morale delay1 drives observation.
- **Зачем:** native BlockedRoute commits new order/revision без movement: NotApplied сохраняет real fence, goal_applied=false, blocked retry допустим, terminal idle требует Applied. Capture eligibility — Routing+committed Attack provenance, не manual Retreat.
- **Где:** `../solaris-default-plugins/sources/solaris-settlements/src/morale.rs` (`OperationalMorale::{advance,complete_control,is_routed_attack}`), `morale_runtime.rs` (`Settlements::{morale_tick,morale_get_answer,morale_operation_answer}`).
- **Доказательство:** IRC `PluginMoraleBoundary`, NativeStructures/capture/hire coordination, source receipts.
- **Статус:** implementation, no midflight gates; final lead/Main validation pending.

### S03 — Capture custody после real cancellations; compensation свежими fences

- **Что:** actual query health/max_health/pose + controls → durable CancelWork→CancelOrder; old native capture делал Idle, не Follow/Hold. Custody CAS только после accepted receipts, fresh controls, living reach ≤3 blocks и social/routed eligibility.
- **Зачем:** не overwrite newer accepted work/order и не replay completed units. Compensation сохраняет canonical remaining work/original order, durable restoration IDs; refusal releases capturing exclusion, сохраняет original role+capture_recovery, fresh attempt получает новые IDs вместо retries rejected immutable IDs.
- **Где:** `../solaris-default-plugins/sources/solaris-settlements/src/capture_runtime.rs` (`CaptureIntent`, `PendingCapture`, `capture_operation_answer`, `capture_get_answer`), `capture_tests.rs`, shared `lib.rs`.
- **Доказательство:** IRC `PluginCaptureBoundary`/`PluginMoraleBoundary`/`PluginEngineConsumer`, edit receipts `history://SettlementsCP049.PluginCaptureBoundary`. Morale rebind использует actual final Controls fence, surrender actual fresh health/pose + real idle proof; pre-hire civil_work/cargo/gear/identity/employer unchanged.
- **Статус:** authored consumer custody/replay/refusal/compensation/CAS/reach/conservation tests, not executed. Recovery no-next chains evacuation recovery, но evacuation acceptance не заявлена.
- **Correction S09:** прежнее описание восстановления работы fresh AssignWork/new remaining-only plan **superseded**. Fresh compensation saga ID не означает новый admitted work ID: нужен explicit Resume **того же** `Controls.work.operation_id`, retained full planned/done history и fresh control fence. Native Cancelled Resume gap пока open.

### S04 — Hire/demob сохраняют remaining civilian work и единственного item owner

- **Что:** CivilWork stores canonical pending_work request+remaining units+cargo fence. Hire persists PauseWork до CancelWork, fresh gear/carry fences до IssueKit, RestoreCivilWork до rollback AssignWork. Demob persists remaining Step::Restore до resume; same NPC, only issued kit returned; employment/civil_work/morale clear после valid work receipt.
- **Зачем:** исключить parallel civilian production, replay completed units, lost intents и duplicate items.
- **Где:** `../solaris-default-plugins/sources/solaris-settlements/src/hire_runtime.rs` (`CivilWork`, `HireIntentStep`, `issue_hire_step`), `demob_runtime.rs` (`Step`, `dismiss_operation_answer`), `hire_tests.rs`, `demob_tests.rs`.
- **Доказательство:** IRC `PluginInventoryConsumers`; `artifact://2177`/`artifact://2191`; authored `hiring_pauses_remaining_civil_work_durably_before_any_military_item_move`, `dismissed_soldier_returns_only_issued_gear_and_recovers_after_reload`.
- **Статус:** implementation/authored tests, not executed midflight, final combined gate pending.

### S05 — Product acceptance moved к shipped Wasm + wire clients

- **Что:** real shipped Wasm + mc_net::bind_with_scripts + wire clients вместо private product DTO/mock prerequisites. Manual workshop damage covers table, unlit campfire, both door halves, container, obstructed route, foreign owner, cold unloaded restart; accounting 3 logs→8 planks+1 log за два успеха, failed attempts не consume/produce.
- **Зачем:** product policy acceptance принадлежит plugin, native authority только physical worksite/container/NPC.
- **Где:** `../solaris-default-plugins/sources/integration-tests/tests/settlement_component_haul.rs` (`certified_manual_workshop_refuses_damage_repairs_and_survives_unload_restart`, `shipped_gate_previews_match_the_catalog_and_every_staged_block_across_chunk_edges`), `settlement_wasm_readiness.rs` (`settlement_vanilla_village_warehouse_funds_and_commits_through_wasm`), `tests/support/settlement.rs`.
- **Доказательство:** IRC `PluginIntegrationBoundary`; source implementations, originals `../solaris-default-plugins/.analysis/codex-logs/cp049/domain-test-handoff`.
- **Статус:** authored, final run pending. Runner uses SOLARIS_CORE_ROOT + SOLARIS_DEFAULT_PLUGINS_ROOT, temporary absolute manifest. Scope CP049, no evacuation coverage claim.

### S06 — Paid shipment genuine principal/escrow durable, payout пока native-unreachable

- **Что:** plugin `payment_mandate{,_runtime}.rs` authored explicit genuine buyer destinationContainer emeraldreservation, original `pm_<shipment>_escrow/payout` IDs, immutable fullcomponent/gross/nativeloss blobs ≤4096bytes/value plus singleCAS. Keys `paid-shipment-v1/<id>`/`paid-blob-v1/<digest>/<fragment>` сохраняют originalactualnative gross/loss.
- **Зачем:** no fake principal0/onlineproxy/cancel-then-pretendpayout. Payment revocation policy должен совпасть с atomicnative inventorycommit, не check-before-IO с race.
- **Где:** `../solaris-default-plugins/sources/solaris-settlements/src/payment_mandate.rs`, `payment_mandate_runtime.rs`, grossprovider `caravan.rs`; `/home/kaiserroman/solaris-settle/crates/mc-simulation/src/services/npcs/routes.rs`, `sdk/rust/solaris-plugin-sdk/src/inventory_ops.rs`.
- **Доказательство:** IRC `SettlementsCP049.PluginPaidShipment` source-only: routesincoming authorizes толькоPrepared, **не Arrived**; Transfer/ConsumeReservation DTO имеет inventoryexpectedfences, **не pluginstorage expected-version condition** для atomic revocation-safepolicy. Exactpaths verified; nativepaymentreceipt отсутствует, workercommands/checks не запускались.
- **Статус:** **NO PASS / payout unreachable текущимnativeAPI**. Root+CP053 owns integration, genericArrivedauthorization+pluginversion atomiccondition нужны до настоящего payout; сохранённая escrowreservation **не payment acceptance**.

### S07 — CP058 war ruleset numeric proposal до source tests, не owner-approved

- **Что:** `CONTENT_SPEC`§21 beforetests фиксирует proposal≤8knownlawfultargets/≤8half-open absolute simulationtickintervals/≤4096UTF8record, grace/window durations **без defaults** — explicit campaignadmission. Послеpeace/revocation0newdamage/destructioncommits, oldcommittedreceipts не refund/replay; offlineprivateprotection explicitonly/unknownfailclosed.
- **Зачем:** исключить guessed offlinePvP/calendarcatchup/hiddenstockroster и cachedpermission между revocation+physicalcommit. Numericproposal должен предшествовать scenarios, но не подменяет ownerapproval или nativeguard.
- **Где:** `../solaris-default-plugins/docs/settlements/CONTENT_SPEC.md`§21 «CP-058: предложение числовой границы war ruleset».
- **Доказательство:** displayedproposalтаблица/explicitnotapproved/noPASS boundary; IRC `SettlementsCP049.PluginWarAuthority`: no commandsrun, native conditionalprecommitstorage-version+causalUUID/tick prerequisites reportedlead.
- **Статус:** **proposal/acceptancerequired-unrun**, active только explicitadmittedruleset withpoliticalprovenance, server-onlyoutsidecampaign unchanged. Predicate/tests сами по себе **не nativephysicalguard**; без atomicconditionphysicalowner refuses, не appliescachedpermission.

### S08 — PaidGuardRelaunch: original scope guards и status-first reserve, пока source-only

- **Что:** Terms/Mandate используют actual buyer scope и **отдельные зарегистрированные recipient scope/UUID**. Terms сохраняют исходные scope versions; `escrowReserve` сначала читает status, и только `NotFound` выпускает original-ID request с исходными four-scope guards и guard фактического positive paid head. Existing common guards/HookResult ABI migrated в source: native request `answer` принимает два аргумента, host разбирает `answer.decision`, SDK wrappers возвращают Result.
- **Зачем:** повторный вызов не должен выдавать новую escrow operation или подменять retained admission свежими scope versions. Общий guard contract не заменяется paid-specific bypass; это **не** разрешение same-carrier-only shortcut или payout без genuine independent principal.
- **Где:** `../solaris-default-plugins/sources/solaris-settlements/src/{payment_mandate,payment_mandate_runtime}.rs`; core root `/home/kaiserroman/solaris-settle/`: `crates/mc-script/src/{operations,precommit}.rs`, `crates/mc-script/wit/{types,domain-operations,precommit}.wit`; `crates/mc-plugin-host/src/{adapter,instance,host}.rs`; `sdk/rust/solaris-plugin-sdk/src/{lib,glue}.rs`, `sdk/rust/examples/hello/src/lib.rs`.
- **Native condition anchors:** в том же core root `crates/mc-entity/src/native_decision_fence.rs`, `crates/mc-entity/src/regional/physical.rs`; `crates/mc-simulation/src/services/storage/{records,version_guards}.rs`; `crates/mc-net/src/script/storage/native_combat.rs`, `crates/mc-net/src/script/storage/native_combat/attacks.rs`. Наличие source anchors само по себе не доказывает atomic physical acceptance.
- **Доказательство:** IRC `SettlementsCP049.PaidGuardRelaunch`, source-edit receipts/transcript (Terms snapshot23DE/runtime E898), verified owning paths. Parent observed compiler **FAIL101 до final ABI source migration**: `/home/kaiserroman/solaris-settle/.analysis/codex-logs/settle/cp049/placement-room-repair.log` (arity и result/decision mismatch, также другие compiler errors). Worker не запускал commands/checks/tests/GUI; отдельного child receipt или runtime migration timestamp нет.
- **Статус:** **source-only / NO PASS**, post-migration rerun не заявлен. Native endpoint-vector и genuine independent-principal acceptance остаются open по parent scope hold; эта запись не закрывает payout blocker S06.

### S09 — Capture compensation: same-lineage explicit Resume, не remaining-only AssignWork

- **Что:** `capture_runtime` сохраняет `prior_work_operation_id` из actual `Controls.work.operation_id`; outstanding work без real admitted ID отказывается **до** capture intent/cancellation. Compensation вызывает `resume_npc_work` с fresh saga operation ID, **SAME original admitted work ID** и actual current control fence; не создаёт fresh AssignWork и не reset completed units.
- **Зачем:** remaining work не новый recipe/job. Receipt обязан совпасть с original ID, быть nonCancelled, `done ≤ planned`, `remaining ≤ saved canonical units`. Consumer fixture сохраняет full plan8/done5/remaining3, **не** plan3/done0; ResumeWork request не manufactures recipe/work units.
- **Где:** `../solaris-default-plugins/sources/solaris-settlements/src/{capture_runtime,capture_tests}.rs`; core root `/home/kaiserroman/solaris-settle/`: `crates/mc-net/src/script/storage/npc_order_execution.rs` (`ResumeWork`), `sdk/rust/solaris-plugin-sdk/src/npc_ops.rs` (`resume_npc_work`); follow-up edit receipts `history://SettlementsCP049.PluginCaptureBoundary`.
- **Исторический native gap / superseded proposal:** тогда actual `ResumeWork` rejects `OperationConflict` при ID mismatch **или state==Cancelled**; Cancelled job сохраняет ID/recipe/planned/done/receipts. Shared `assign_npc_work` resume mode сохранял original work/progress/receipts и checks real record fence/live owned actor. Предложенный narrow **explicit cancelled-Resume gate** был HOLD и затем **не реализован / отвергнут**: **Cancelled remains terminal**, automatic skip unchanged. Новый approved generic **SuspendWork→Resume SAME original lineage/current fence** и source capture handoff — **S11/S12**, native consumer proof S10; это **не revival Cancelled**. Plugin compensation runtime proof ещё pending.
- **Доказательство:** IRC `SettlementsCP049.PluginCaptureBoundary`, exact live source read executor245–272/rejection257–260, SDK162–177: request envelope ResumeWork(admitted_operation_id,expected_revision). Owning paths verified; worker runtime/check commands during exclusive perf window не запускал.
- **Статус:** эта историческая plugin lineage migration не была functioning compensation; typed rejection releases exclusion/original role/no custody и durable capture_recovery были fail-closed handling. **Cancelled-work restoration не поддерживается и не является pending authorized behavior**. Новый suspension contract имеет native consumer и **16 plugin BUSINESS UNIT PASS (S12)**; **actual Wasmtime/live-native-owner capture пока НЕ exercised**. Supersedes S03 remaining-only AssignWork detail, не substitutes synthetic work receipt.

### S10 — Native living clock: sparse damaged timers, owned kernel order и no double aging

- **Что:** `mc-entity/runtime.rs` хранит sparse BTreeMap **nonzero living timers**: damage/rehydration регистрирует, zero удаляется. Damaged-only `LivingTimerClock` component **не меняет idle LivingState layout**; нет all-actor timer scan и per-tick collection allocation.
- **Механизм:** существующий living kernel исполняется **один раз per selected owned tick**, separate route clock removed. `TickMode::EffectsOnly` пропускает duplicate **nonplayer** living advance; Player остаётся **Normal+External**.
- **Зачем/source order:** actual local client.jar `version.json=26.1.2`; decompiled `LivingEntity.java::baseTick`468–499 уменьшает hurt timer, затем invulnerability timer **nonServerPlayer до effects**; `aiStep` не источник этого aging. Сведение clocks к kernel сохраняет order и позволяет guard recover без отдельного route-only aging.
- **Где:** root `/home/kaiserroman/solaris-settle/`, `crates/mc-entity/src/runtime.rs`, `crates/mc-entity/src/runtime_26_1_2/transaction.rs`; narrow correction в `docs/evidence/phase5-entity-damage-api-2026-08-26.md`.
- **Доказательство:** `.analysis/codex-logs/settle/cp049/<slug>/{result.json,run.log}` относительно того же root: `native-living-sparse-layout-consumers` **2 PASS**, `native-living-kernel-clock-compatibility` **34 PASS**, `nativecombat-owned-living-clock-final` **7 actual owned-world combat PASS**, включая previously failing guard; `native-work-suspension-custody-consumer` **1 actual suspension/craft custody PASS**, Cancelled remains terminal. Все four result status=passed/exit0, counts verified в focused logs; nice19/−j3.
- **Статус:** focused runtime/consumer proofs; **не GUI/performance/L2/CP closure**, не standalone proof plugin capture compensation. S09 historical rejected cancelled Resume не превращается автоматически в accepted route.

### S11 — Approved generic SuspendWork ABI: same-lineage Resume, terminal Cancel и no automatic continuation

- **Что:** новая approved **generic operation**, ABI **не frozen**. `SuspendWork` даёт `Suspended` **SAME admitted intent**, сохраняет original work lineage; reason opaque, не native employment/custody policy. Public Resume gate — **только SAME original Suspended**, Cancelled остаётся **terminal**, cancelled-resume proposal не реализован.
- **SDK contract:** `suspend_npc_work` exact six args: **request, newOperationID, actorHandle, ORIGINALadmittedID, opaqueReason, expectedRevision**. Fresh command operation ID не заменяет admitted work ID.
- **Native transition:** suspend ставит Suspended/reason, очищает deadline+hash/wait; private due continuation excludes **Suspended+Cancelled**. Cancel очищает suspension label. Stopped-state bounds/validation согласованы в DTO/WIT; no automatic wake.
- **Где:** root `/home/kaiserroman/solaris-settle/`; `crates/mc-script/src/npc_order_operations.rs`593/725/893/1029/1091–1095; `crates/mc-script/wit/npcs.wit`398 record/536 variant; `sdk/rust/solaris-plugin-sdk/src/npc_ops.rs`168. Executor `crates/mc-net/src/script/storage/npc_order_execution.rs`247 dispatch/974 suspend/999–1001 transition/271–274 public Resume/611 private due/1039 Cancel.
- **Доказательство:** IRC SettlementsCP049 approved contract/source anchors; paths verified. `.analysis/codex-logs/settle/cp049/native-work-suspension-custody-consumer/{result.json,run.log}` **1 actual native craft/custody/reopen/producer/ACK consumer PASS/exit0** (S10), не mock echo.
- **Статус:** этот native consumer proof отделён от plugin BUSINESS UNIT proofs **capture16 (S12), hire8/demob5 (S13)**; **actual Wasmtime/live-native-owner capture не exercised**, не GUI/L2/CP closure. Initial hire7PASS/1FAIL и demob4PASS/1FAIL superseded final S13, не runtime-contract relaxation.

### S12 — Final authorized capture saga: Suspend before custody, terminal Cancel after grounded CAS

- **Что:** Main superseded **Cancel-before-CAS / Cancelled-resume debate**. Final source до custody CAS вызывает actual **SuspendWork SAME admitted `Controls.work.operation_id`** с opaque custody-review reason и actual **CancelOrder**. При refusal сначала durable **Compensate**, затем fresh controls и original-order restoration/**Resume SAME admitted ID**.
- **После custody:** successful custody CAS делает terminal **CancelWork**, **NEVER Resume Cancelled / rollback grounded custody**. `CaptureIntent` сохраняет post-CAS CancelWork/CancelNeeded checkpoint, пока `captive_by=Some/role=None`; cleanup CAS очищает intent **только после actual SAME original WorkAssignment terminal observation** (Cancelled либо Committed и pending_work=None).
- **Retry/precedence:** failed terminal cancel exposes pending/refusal; unchanged rejected record fence **не reissue/new ID**. Different lineage — **не proof и не cancel target**; new attempt только same stopped original с newly observed actual record fence. Already Suspended employment work сохраняет actual prior reason; capture refusal **не wakes it**.
- **Surface/identity:** RootFinish shared roster CLI/Overview/Squads интегрирует `terminal_cleanup_pending`/refusal getters. Source fixtures сохраняют original role/identity/cargo/civil intent; source replay covers before/after custody CAS, durable CAS refusal compensation, original-vs-newer terminal work, native-fence retry precedence, prior suspension; **не wording tests**.
- **Где/receipt:** `../solaris-default-plugins/sources/solaris-settlements/src/{capture_runtime,capture_tests}.rs`; source handoff `history://SettlementsCP049.PluginCaptureBoundary`, parent approved generic contract S11/native consumer S10.
- **Точный proved selector:** cwd `/home/kaiserroman/solaris-default-plugins`, selected settle SDK; `nice -n 19 cargo test --manifest-path /home/kaiserroman/solaris-settle/.analysis/codex-logs/settle/cp049/plugin-unit/Cargo.toml --lib capture_tests:: -j 3 -- --nocapture --test-threads=1`.
- **Initial diagnosis:** parent IRC report **15 PASS /1 FAIL**, после compile cutover `state_name` arm Suspended. Caller-tracked diagnostic локализовал redundant second `test_registry_read` в `capture_tests.rs`611: canonical registry helper **уже cascades startup recovery into CAS publication**. Удалено obsolete extra-read assumption, **не capture_runtime/contract relaxation**; role/social/remaining-work/original-ID/fresh-fence/rejected-ID/reopen/role-change assertions сохранены, temporary track_caller removed/registry helper reverted byte-exact. Initial evidence — child history, final proof ниже supersedes pending result.
- **Final proof/status:** `/home/kaiserroman/solaris-settle/.analysis/codex-logs/settle/cp049/plugin-capture-suspension-custody-consumers-final/{result.json,run.log}` — **exit0 /all16 capture BUSINESS UNIT PASS**, включая corrected refused-compensation case; receipt command/cwd/count verified. Child source handoff **NO additional checks/GUI/format**; lead ran selector. **Не actual Wasmtime/live-native-owner capture/GUI/L2/CP closure**, не Cancelled resurrection; source saga invariants выше unchanged.

### S13 — Hire/demob suspension business proofs: реальные fixture owner/payment/cancel boundaries

- **Hire root cause:** canonical MILITIA hireplan legitimately rejected **occupied signing-payment slot0**. В `hire_tests.rs` civil `oak_log` перемещён **0→1** в двух fixtures (initial549/post-suspend679), освобождая actual wage slot0; **runtime/fee contract не изменён**.
- **Demob root cause:** old fake owner rejected admission, test затем ошибочно feed captureQuery refusal. В `demob_tests.rs` используется actual **`attacker` / oak_hollow owner** (line525); добавлено actual **CancelOrder original dead resident /order revision11** observation (558–562) **до feeding NotFound**. Не manufacturing independent principal/не bypass ownership guards.
- **Где:** `../solaris-default-plugins/sources/solaris-settlements/src/{hire_tests,demob_tests}.rs`; all original durable intent/gear/ownership/capture/suspension/reopen assertions retained.
- **Доказательство:** `/home/kaiserroman/solaris-settle/.analysis/codex-logs/settle/cp049/plugin-{hire,demob}-suspension-consumers-final/{result.json,run.log}` — **hire8 PASS /demob5 PASS, both exit0**; source paths/receipt command/count verified. Cwd plugins, nice19, selected settle SDK через `plugin-unit/Cargo.toml`, `--lib {hire_tests::,demob_tests::} -j3 -- --nocapture --test-threads=1`, targetdir settle/target.
- **Статус:** **BUSINESS UNIT ONLY**, supersedes initial hire7/1 and demob4/1 FAIL. **Не actual Wasmtime/live-native-owner/GUI/L2/CP acceptance**; fixture fixes не новая runtime behavior.

## Tooling+process

### P01 — Тесты должны исполнять production path, а не legacy authority

- **Что:** удалены herd legacy queue/API и `WorldBusy` test-only tick path; fixtures используют production spawn/scheduler/commit. Block-edit fixtures запускают настоящий async owner на OS thread/current-thread runtime; production lock wait проверяется `async_owner_wakes_on_world_unlock_and_commits_once`. Одинаковые one-shot race pause probes собраны в `TestPause` + mutex map; data/async-specific probes не унифицированы искусственно.
- **Зачем:** зелёный test-only путь не доказывает production authority, а 27 однотипных pause points не требуют 27 отдельных механизмов.
- **Где:** `crates/mc-net/src/play/block_edit_commit.rs`, `crates/mc-net/src/play/session.rs` (`TestPause`, `install_test_pause`, `pause_for_test`).
- **Доказательство:** `.analysis/codex-logs/test-path/receipt.md`: 32-file port +562/−2173; mc-net 2072 passed / 6 ignored; L2 correctness passed; TestPathReview 0 findings; probe consolidation 81 call sites, focused Clippy clean. Orphan legacy source также удалён.
- **Статус:** production-path port и probe consolidation интегрированы; source-path-only checkpoint не объявлен gameplay/client gate.

### P02 — Lean worktrees после ENOSPC; build bytes не копировать

- **Что:** владелец предписал `.analysis/tools/agent-worktree.sh`: source snapshot без `target`, `sdk/rust/target`, `tmp` и `.analysis`; отдельный `.analysis/codex-logs`. Main `target/` однажды достиг 164G и вызвал ENOSPC. Helper формирует staged baseline; агентская работа остаётся unstaged, самостоятельно staging/stash/reset/commits не выполняются.
- **Зачем:** параллельные compile/debug artifacts исчерпывают shared disk и дают phantom failures, а не дефекты реализуемого механизма. Helper — владелец baseline plumbing, не разрешение агентам менять index.
- **Где:** `.analysis/tools/agent-worktree.sh:1-19`, `AGENTS.md:212-243`.
- **Доказательство:** директива владельца 2026-10-01; `.analysis/codex-logs/worldgen-w1/receipt.md` документирует ENOSPC и очистку только reproducible worktree target (4.9 GiB), затем debug/test symbols и incremental disabled.
- **Статус:** действующий process; no-symbols build не является release optimization/performance claim. Main-owned integrated validation запускается один раз после integration.

### P03 — Один автор журнала, остальные сообщают факты

- **Что:** ARCHIVIST — единственный writer этого log и его index row; workers читают и присылают `record:` по IRC. `docs/MEMORY.md` остаётся Main-owned, ADR — у owner изменения архитектуры. Записи дедуплицируются, cited paths проверяются, status не повышается без evidence.
- **Зачем:** не терять механизм/решение/receipt между сессиями и не получать конкурирующие historical cursors.
- **Где:** `docs/memory/log-18-2026-10-parallel-tracks.md`, `docs/memory/README.md`.
- **Доказательство:** контракт ARCHIVIST и постановка 2026-10-01. `learn` не смонтирован; доступный durable-memory эквивалент — `xd://retain` (сообщено Main).
- **Статус:** standing archival writer; закрывается только сообщением Main `archivist: close`.

### P04 — CleanWorld: transaction admission вместо publish test hook

- **Что:** ResidentChunkTransaction держит exclusive ResidentPublicationState admission через persist/publication; worker заменяет test-only publish_hook двух тестов на pause в existing commit_durably persist callback.
- **Зачем:** production proof concurrent readers получают обе committed values, precommit-token mutation stale; extra hook не нужен.
- **Где:** `crates/mc-world/src/resident.rs` (`ResidentChunkTransaction`, `set_publish_hook`, `publish`, `commit_durably`; worker inspection 3224–3373).
- **Доказательство:** IRC `CleanWorld`, CodeGraph/source inspection, final receipt ещё pending.
- **Статус:** cleanup в работе, smoke/pass не заявлен.

### P05 — CleanCore: private test support вместо production helpers

- **Что:** single-caller EntityTypeFlags::bits удаляется, child test читает private tuple .0. NBT test-only read_string +36 tests moved в sibling nbt_tests; physics GridPos нужен только LocalBlocks inline fixtures, не aquatic tests, moved private physics_tests без shared support/visibility widening.
- **Зачем:** убрать лишние production cfg(test) helpers, сохранив реальные decoder/physics paths.
- **Где:** `crates/mc-data/src/entity_contract_26_1_2_tests.rs`, `crates/mc-nbt/src/nbt_tests.rs`, `crates/mc-physics/src/physics_tests.rs`.
- **Доказательство:** IRC `CleanCore`/`CleanCore.CorePhysics`, source inspection; no allow(dead_code)/allow(unused) в пяти owning crates. Final receipt pending.
- **Статус:** mechanical implementation, parent validation pending.

### P06 — Protocol wrapper removal и настоящий encryption owner

- **Что:** private cfg(test) HashedStackComponentHashes::decode single caller заменён прямым unchanged decode_with_budget с identical zero budget. Frame docs corrected nonexistent crate::cipher/M1.d scaffold: framing plaintext, AES-128/CFB8 owner MinecraftCipher в mc-net, inbound/outbound states independent.
- **Зачем:** убрать ложные helper API/архитектурные ссылки без изменения decode budget/packet format.
- **Где:** `crates/mc-protocol/src/packets/play/tests/serverbound_and_slots.rs`; `crates/mc-protocol/src/frame.rs`, `crates/mc-net/src/encryption.rs` (`MinecraftCipher`).
- **Доказательство:** IRC `CleanCore.CoreProtocol`/`CleanCore`; перенесённый после удаления child worktree receipt `.analysis/codex-logs/clean-core/protocol-receipt.json`; extracted bodies retain indentation for exact bytes.
- **Статус:** mechanical/source proof, no midflight builds/tests.

### P07 — Harness sibling tests не accidental binaries

- **Что:** parity/replay/coverage inline tests moved sibling files с cfg(test)/same module/private access. Autobins=false нужен для src/bin/coverage_audit_tests.rs, четыре executable targets явно объявлены. Retained4+27+7=38 tests, четыре tautology/wording/help-substring/ledger-snapshot tests removed, unused PNG removed; follow-up removes pure wording/nonempty-action assertions, сохраняет expect_err/substantive comparisons.
- **Зачем:** Cargo discovery не должен создавать binary из test support; incidental wording не behavior proof.
- **Где:** после child integration/removal `crates/mc-test-harness/Cargo.toml`, там же `src/parity_tests.rs`, `src/replay_tests.rs`, `src/bin/coverage_audit_tests.rs`.
- **Доказательство:** IRC `CleanHarness.HarnessTestsCleanup`, exact inverse-indent roundtrip/final byte comparison после declared assertion surgery; final diff+1268/-1238 across10 files. Tool-save formatting undone.
- **Консолидированный patch:** `.analysis/codex-logs/clean-harness/receipt.md`, подтверждение lead `CleanHarness`; scoped gates pending.
- **Статус:** mechanical proof only, no build/test/fmt; parent gates pending.

### P08 — Xtask ownership tables extracted без изменения scanner

- **Что:** OwnershipRule +170 ordered rules moved private child, девять pub(super) additions (type/six fields/two tables); scanners/dispatch/gates/messages byte-exact. Parent4115→2744 +child1374 lines.
- **Зачем:** отделить ownership data без изменения enforcement/новой архитектуры.
- **Где:** после child integration/removal `crates/xtask/src/code_health/ownership_rules.rs`.
- **Доказательство:** IRC `CleanHarness.XtaskOwnerCleanup`; stripping9 prefixes equals original67265-byte slice SHA256 `8316a132db29ec533aa0f2f9fa4a875dda9bf4cb56a05ba61f99b20c3e653e8c`; reverse whole-parent SHA256 `67af725e116d032875bfd425c558353a56786958c24077e2a8c3e3ac605abbd9`.
- **Консолидированный patch:** `.analysis/codex-logs/clean-harness/receipt.md`, подтверждение lead `CleanHarness`; scoped gates pending.
- **Статус:** byte proof only, no gates; tool-autoformatted long string restored.

### P09 — World: move owned vectors, eviction behavior вместо count spy

- **Что:** BlockRegistry::from_report moves schema/state-id vectors в Block и borrows block.properties вместо nested String/Vec clones. Inline test roots moved siblings без module-name/Anvil production changes. read_view count spy не нужен: existing last-release eviction test уже проверяет behavior.
- **Зачем:** убрать avoidable allocation и tautological support без ослабления сохранения/retention.
- **Где:** `crates/mc-world/src/block.rs` (`BlockRegistry::from_report`, inspected188–201), `crates/mc-world/src/storage/read_view.rs` (`retained_read_chunk_is_not_evicted_until_last_release`).
- **Доказательство:** IRC `CleanWorld`; source inspection. Real vanilla test-world fixture absent в main/cleanup; planned save proof synthetic disk/region byte roundtrips, не vanilla fixture claim.
- **Статус:** cleanup ongoing, final receipt/validation pending.

### P10 — Xvfb displayfd EOF нельзя оставлять в selector

- **Что:** harness readiness уже push-driven (wait_port/_wait_for_file inotify+pidfd, client state_version notifications), но start_xvfb повторно выбирал registered displayfd pipe после EOF. EOF теперь terminal announcement failure: producer stop + socket-event fallback; failed startup закрывает log handle.
- **Зачем:** live producer, закрывший announcement FD, вызывал busy-spin без полезной readiness; push-механизм не исключает постоянно ready EOF.
- **Где:** `tools/harness/runtime.py` (`start_xvfb`).
- **Доказательство:** IRC `CleanHarness`; `.analysis/codex-logs/clean-harness/receipt.md` (23451 select calls/0.1s), `xvfb-eof-after.json` (1 select, exit -15), `python-event-smoke.json`.
- **Статус:** targeted before/after scenario verified worker; consolidated receipt/parent gates pending.

### P11 — Exclusive windows относятся к latency, не к любому scale gate

- **Что:** Main разрешил parallel `nice -n 19 -j 2` builds во время текущего TickGoals entity-scale population/correctness gate; living-world-scale latency и dispatch A/B измеряются в announced exclusive windows.
- **Зачем:** не простаивать вокруг correctness gate и одновременно не выдавать contended latency за чистый A/B. Если population gate терпит disconnect/timeout от contention, Main решает exclusive rerun, не worker меняет workload.
- **Где:** policy исполнения harness, `AGENTS.md` validation tiers/receipts; measured трек T01/T02.
- **Доказательство:** IRC Main 2026-10-01 (build policy, после завершения living-world latency window).
- **Статус:** историческое policy уточнение; TickGoals сообщил final entity-scale passed, no contention rerun needed. Latest Main P12 разрешает builds2..3 и меняет dispatch на phase profiling; final combined living-world latency всё ещё exclusive.

### P12 — Async shell deadline и пересмотр build window

- **Что:** Main установил: async bash сохраняет default timeout300s, если явно не передан timeout; long cargo commands задают deadline, например3600. После TickGoals windows closed/entity-scale passed builds разрешены nice -n19 -j2..3; Main dispatch измеряется in-process phase profile, final combined living-world latency позднее announced exclusive window.
- **Зачем:** async не отменяет deadline; корректный expensive build не должен случайно завершаться timeout. In-process phase profile и canonical latency имеют разные noise/acceptance границы.
- **Где:** invocation policy bash/harness, `AGENTS.md` validation scope.
- **Доказательство:** IRC Main + TickGoals 2026-10-01; latest build-open instruction supersedes earlier hold/exclusive dispatch announcement.
- **Статус:** current checkpoint policy, final combined latency pending.

### P13 — CleanCore final: реальные consumer bytes/physics, focused gates passed

- **Что:** завершены P05/P06 и item-components extraction; NBT1911→1121 lines/36 tests, physics3214→1512/58 tests, codec41/frame30 test attributes, item-components9 tests с raw JSON bytes preserved. mc-domain intentionally unchanged; public mc-data testing fixtures остаются, поскольку их используют другие crates.
- **Зачем:** test-owner extraction не меняет production APIs/runtime algorithms; shared fixture нельзя blind cfg(test)-gate. NBT aggregate/allocation budget tests сохраняются как реальные parser invariants.
- **Где:** `crates/mc-nbt/src/nbt_tests.rs`, `crates/mc-physics/src/physics_tests.rs`; protocol/data owners описаны P05/P06.
- **Доказательство:** `.analysis/codex-logs/clean-core/receipt.md`: independent read-only review pass; scoped all-targets пять crates768 passed/25 existing ignored, strict Clippy pass, canonical code-health pass, owned-only fmt/raw-string fidelity. Throwaway real consumer filesystem item-components + plain/compressed frames carrying Modified UTF-8 NBT + falling-body floor collision exit0; scaffold removed.
- **Статус:** focused gates/smoke passed, supersedes P05/P06 pending; receipt overall verdict changes (готовый patch для Main), не full L2/gameplay/client/scale/latency acceptance. Patch SHA256 `d02c51156362358d22a96e38728bab78f3d1c94353b1d9ccd24be31f21101806`.

### P14 — Client stimulus20Hz — не readiness polling

- **Что:** Main approved две50ms tokio intervals: send_client_ticks_for и drain_entity_scale_client создают20Hz client stimulus. Добавлено rationale, параметры/semantics/budgets неизменны.
- **Зачем:** заменить benchmark input на server-tick push значит снижать offered load при lag и менять смысл измерения; event-driven readiness contract не запрещает pacing workload.
- **Где:** `crates/mc-test-harness/src/bin/wire_probe.rs` (`send_client_ticks_for`), `tests/load_scenarios.rs` (`drain_entity_scale_client`).
- **Доказательство:** IRC `CleanHarness`; `.analysis/codex-logs/clean-harness/receipt.md` summary0 fail/verdict KEEP.
- **Статус:** Main-approved explicit pacing boundary; code-health passed, не scale/client/performance proof.

### P15 — CleanWorld final: сохранение real report/Anvil bytes доказано

- **Что:** P04/P09 finished,156 moved test identities в8 roots, семь closures preserve captured code ignoring fmt/optional commas; production roots16814→10884 lines — navigation, не compile/performance improvement. Anvil production prefixes byte-identical baseline; public durable callback tests заменяют internal between-owner pause, readers обе committed values/precommit token stale.
- **Зачем:** format compatibility подтверждать actual bytes, а не уменьшением root/зелёным compiler. Test boundary теперь public durable admission, не прежняя искусственная pause точка.
- **Где:** `crates/mc-world/src/resident.rs`, `crates/mc-world/src/block.rs`, `crates/mc-world/src/storage/read_view.rs`; owning Anvil paths и moves в receipt.
- **Доказательство:** `.analysis/codex-logs/clean-world/receipt.md`: mc-world all-targets302 passed/15 ignored, strict Clippy и code-health pass, real vanilla block report29873 states passed, independent review no findings. External real-report consumer chunk(-2,5), Y-64/319, inhabited_time37 wrote/read/rewrite real NBT/region; both12288-byte regions SHA256 `4ed4ec53a02ff701178fd30a3f3532847d2894ca8e869f91e89ab52fc40c7e73`.
- **Статус:** final scope pass, supersedes P04/P09 pending. No full L2/scale; real vanilla region/LZ4 fixtures отсутствуют, **не** real-vanilla-region acceptance. Excluded native/storage.rs/world_lease/atomic_file untouched; replace_for_test callers в excluded storage.rs оставлены по scope, не total-purge claim.

### P16 — CleanHarness final: false-green boundaries без message pins

- **Что:** P07/P08/P10 completed; filesystem event wait+Linux watcher setup имеют одного owner, backend direct-script mode retained. Review обнаружил, что удаление wording tests потеряло no-tests/ignored-only/failed-count/missing-summary rejection coverage; она восстановлена через actual scenario exit/receipt status matrix, не error strings.
- **Зачем:** incidental wording удалить можно, consumer-visible false-green invariant нельзя; EOF/process cleanup regression использует real child.
- **Где:** `tools/harness/test_harness.py` (consumer rejection/status), `tools/harness/runtime.py` и harness/xtask owners P07/P08.
- **Доказательство:** `.analysis/codex-logs/clean-harness/receipt.md`: all-targets245 passed/0failed/128ignored across40 binaries, strict Clippy clean, owned-only fmt, canonical code-health0fail/KEEP, harness-check17 Python+10shell syntax passed, list25profiles. Actual atomic inotify file publication/present-file/timeout and real Xvfb startup-stop(:0 exit0), direct driver CLI and coverage-audit smoke passed.
- **Статус:** final scoped pass, supersedes earlier pending; child worktrees removed. No full L2/scale/latency/gameplay/client proof. Patch SHA256 `62935294e832a5e8cf49c96ac70967fa2e594b5eac5ec92898d3448d7578d056`.

### P17 — Net cleanup inventory: delete obsolete path, не preserve policy extraction

- **Что:** read-only14 ranked items snapshot play14058/simulation8599. Proven zero-prod-caller cfg(test) bounded_entity_ids_due_for_tick/radix_sort_entity_ids/entity_goal_ids_due_for_tick superseded; delete obsolete cohort tests, но retain live movement-tracking tests. Duplicate Network-NBT text encoders имеют11production+1test caller. process_batch984LOC/seven callers, process_envelopes four callers — реальные contract groups, не command-bus rewrite.
- **Зачем:** WAIT/MIXED boundaries не разрешают сохранить settlement/script policy mechanical rename/extraction. CP040 order: PlayerRegistry split → semantic effects → owner/tick move; CP050 composition/surface later. Spy request counts/route mirror можно убрать только сохраняя stale/durability/race outcomes.
- **Где:** `crates/mc-net/src/play.rs` (obsolete739–828), `configuration.rs` (text626), `play/simulation.rs` (process_batch6901–7884), `loader.rs`; entity `group_orders.rs`/`villager_population.rs`/`natural_spawn_26_1_2.rs` в том же worktree crate.
- **Доказательство:** `.analysis/codex-logs/clean-core/mcnet-inventory.md`, `mcnet-inventory-source-identity.json`; bounded source/caller/CodeGraph inspection, no code edits/gates.
- **Статус:** candidate inventory only, anchors must revalidate after live purge. Loader allow(dead_code) discriminator fields **не blind-delete**: deny_unknown_fields отвергнет valid payload. FMA helpers group/villager одинаковы, natural_spawn nonFMA — объединение всех изменит rounding. No allocation/performance claim.
- **Review clarification CleanCore:** Loader bytes parsed **один раз** from_slice, затем existing Value deserialized from_value — не второй parse payload bytes. Лишний owned discriminator copy/three fields остаются; process_batch7 — invocation sites, не definitions. IRC сообщает independent source-identity review unchanged; no runtime measurements/builds.

### P18 — CleanNet1: одна Network-NBT adapter, production timeout под paused time

- **Что:** revalidated11 text_component_nbt production calls (1Configuration/9Play/1generic command feedback) объединяются у connection wire adapter. Connection+Play5s deadline вместо cfg(test)10ms; direct/outbound stall scenarios start_paused=true, wrappers production deadline+1s вместо real250ms.
- **Зачем:** unit tests должны исполнять тот же timeout policy, не другой prod branch; deterministic paused Tokio time не ждёт wall clock. No settlement/script/plugin code change.
- **Где:** `crates/mc-net/src/connection.rs`, `configuration.rs`, `play.rs`; `play/tests/direct_response_write_stall.rs` (line60), `outbound_write_stall.rs` (line48), `outbound_delivery.rs`.
- **Доказательство:** `.analysis/codex-logs/clean-net1/report.json`, IRC `CleanNet1`: write_stall4 (два actual Play-loop cleanup), connection14, outbound_delivery6, oversizedtext1, decoded Configuration disconnect1 passed; watchdog-only last edit verified1. Injected2s helper test pending1ms-before/timeout-at-expiry. Independent review no findings.
- **Статус:** final R2/R6/R14login-only scope pass; production deadlines5s во всех builds, login stale milestone comment corrected. Aggregate mc-net/Clippy/code-health Main-owned, not run slice. Patch SHA256 `e8b76311703dc5b691a72ad1bc062370640f504b0e5f8524d7a7cdf8d84016a4`.

### P19 — CleanSim test helpers уходят к sibling owner, journal/services исключены

- **Что:** test-only inventory wrappers take_from_slot/decrement_cursor/take_throw_stack и crafting repair_item_crafting_result, embedded block_placement/survival test-root helpers identified; preparing sibling *_tests moves с production bodies preserved.
- **Зачем:** tests не должны жить через лишние production wrapper shims; extraction не должна сохранять plugin domain policy или менять журнал.
- **Где:** `crates/mc-simulation/src/inventory.rs`, `crafting.rs`, `block_placement.rs`, `survival.rs`.
- **Доказательство:** `.analysis/codex-logs/clean-sim/receipt.md`:11placement+15survival test identities/helpers moved,4wrappers removed; four production roots byte-identical except declared removals. All-targets296pass/1ignored, strictClippy/fmt-check pass, independent review no defects; public API smoke double-slab real world snapshot, crafting damage55, hotbar canonicalempty, zombieXP5 exit0.
- **Статус:** final bounded scope pass; tenfile patch SHA256 `47765c661d57113390c5aae36e52b49165a4f842322220f8ab9a767bdd7fe0ac`, Main dependent-lib/code-health/L2 pending. Journals/product/persistence overlap intentionally untouched; inventory temp vectors not optimized без measured bottleneck.

### P20 — Flaky trample был fixture authority mismatch, не production race

- **Что:** fixture simulation owner captured original empty world; test заменил только state.world/world_read. Trample идёт через state.simulation, owner rejects against old world, reply может прийти до first poll. Fix uses same-world controlled owner + exact wait_for_command event, затем mutate stone и process queued guarded edit; control case ordinary farmland→dirt не даёт vacuous pass.
- **Зачем:** удержание replacement-world mutex не блокирует unrelated owner. Tests, swapping state.world, обязаны также связать state.simulation; Ready vs Pending first poll не causal event. Production expected-state+mutation-token write-lock guard unchanged.
- **Где:** `crates/mc-net/src/play/tests.rs` (fixture1189–1204), `crates/mc-net/src/play/tests/plants.rs` (`farmland_trample_does_not_overwrite_a_newer_block_state`), `crates/mc-net/src/play/simulation/request_wait.rs` (64–66).
- **Доказательство:** `.analysis/codex-logs/flaky-trample/report.json`: deterministic wrong-world repro failed101, corrected regression1pass, fixed queue200/200 plant suites×83 tests under8processes (no retries/sleeps), mc-net2074pass/6ignored, strictClippy clean.
- **Статус:** corrected test authority+event sync integrated; no production race fix/gameplay semantics change.

### P21 — Main integration и repo formatter ownership

- **Что:** Main integrated clean-core/world/harness/net1, TickGoals(reviewpass), flaky-trample, EntityIdHasher. Net inventory14items+CP040/050 plans main copy ready, WAIT items после settlement purge. Tools edit/write иногда auto-rustfmt saved Rust вне repo style, создавая unrelated reflow; owning agents нормализуют cargo fmt -p<crate> и восстанавливают только untouched bytes из baseline before patch.
- **Зачем:** tool-induced formatting не user/source change; отсутствие scope fidelity портит параллельную integration. Formatter owner обязан сохранить чужие dirty edits, baseline restore касается только own unintended reflow.
- **Где:** `.analysis/codex-logs/clean-core/mcnet-inventory.md`, owning Rust sources каждого integrated track; repo `AGENTS.md` dirty-tree/validation policy.
- **Доказательство:** IRC Main2026-10-01 integration/tool receipts; ранее exact-byte restoration facts P07/P08.
- **Статус:** patches integrated, не автоматически combined final L2/latency/client acceptance.
- **Main resume integration:** clean-sim/clean-data/clean-test-harness также integrated; canonical receipts `.analysis/codex-logs/clean-sim/receipt.md`, `.analysis/codex-logs/clean-data/receipt.md`, `.analysis/codex-logs/clean-test-harness/receipt.md`. PatchReview2 crafting-anchor finding fixed в `crates/xtask/src/code_health.rs`. TickHash/TickPhys/W2 saved receipts/patches pending apply; no implied gate elevation.
- **Latest Main formatter уточнение:** tick-hash/tick-phys/W2 получены с non-repo tool formatting; Main cargo fmt--all clean. Source workers перед final patch cargo fmt-p owning crates, **не hand-revert baseline lines вне своих изменений**; baseline formatting-only reconciliation Main-owned. Это supersedes прежнюю общую restore-baseline рекомендацию для чужих/untouched lines. ARCHIVIST markdown-only, Cargo не запускает.

### P22 — CleanData: nine test owners, budget helper test-local

- **Что:** nine inline modules147tests moved siblings biomes/block_light/collision_shapes/data/loot/recipes/tags/vanilla_feature_closure/village_data; JsonBudgetLimits::unbounded factory replaced test-local Copy value, APIs unchanged.
- **Зачем:** production roots shed tests/helpers без другой decoder/data authority.
- **Где:** `crates/mc-data/src/biomes_tests.rs` и named siblings; `loot/entity_26_1_2/compile.rs` (budget helper).
- **Доказательство:** `.analysis/codex-logs/clean-data/receipt.md` + move-proof: nine production prefixes byte-identical owner-proof, all-targets319pass/25ignored, strictClippy/fmt-check/workspace-all-targets check/code-health passed, independent review no findings. Real throwaway consumer nestedrecipe stone×4/malformedJSON reject/builtin stone→cobblestone passed, removed.
- **Статус:** final requested scope pass,20files API/source compatible patch SHA256 `c7e950fdf1c9e247a7959918deccfd10d43ce8b5e8a5553abf4a2d497db57929`; initial interrupted code-health not pass. No runtime owner split/unrelated data/dependency changes; final combined gameplay/latency/L2 не заявлены этим slice.

### P23 — Active sessions watch — authority, pressure snapshot lag допустим

- **Что:** RuntimeTelemetryHandle::subscribe_active_sessions сообщает exact register/unregister lifecycle; snapshot().active_sessions — published pressure observation и может lag.
- **Зачем:** удаляя100ms quiet timeout из wait_for_active_sessions, доверять event channel, не assert instantaneous pressure snapshot. Watch0/snapshot1 возможны без lifecycle failure.
- **Где:** `crates/mc-net/src/server.rs` (834–851), `play/session/session_lifecycle.rs` (392–396), `play/session.rs` (1595–1596).
- **Доказательство:** `.analysis/codex-logs/clean-test-harness/receipt.md` (watch0/snapshot1), `session-event-smoke-rejoin.log` actual connect/disconnect/same-profile rejoin exact events passed; IRC `CleanTestHarness`.
- **Статус:** final clean-test-harness scope pass/integrated: lib31pass +17changed targets52pass/107ignored, strictClippy/code-health/fmt-check pass, independent review correct/no findings. Seventeen duplicate keepalive implementations share tests/support/wire_client.rs, handshake reused preserving grounded pose. Watch completes immediately,5s fail-closed deadline unchanged; ignored benches не executed.

### P24 — Native client audit: pin version, не spoof Loader platform

- **Что:** local official26.1.2 descriptor verified client SHA1 `4e618f09a0c649dde3fdf829df443ce0b8831e65`/38113927bytes и server SHA1 `97ccd4c0ed3f81bbb7bfacddd1090b0c56f9bc51`/60417480 match. Embedded protocol775/world4790/resource84.0/data101.1/Java25. Loader HEAD bab2329c707d4bff026f22b65162e93b432ab50c wire3/schema2 поддерживает fabric/neoforge/forge.
- **Зачем:** native platform требует upstream approval, не spoof fabric; official version hashes/layout authority нельзя брать из памяти.
- **Где:** `.analysis/decompile/26.1.2.json`; sibling `../solaris-loader` current platform admission.
- **Доказательство:** `../solaris-client/docs/COMPATIBILITY_AUDIT.md`, `../solaris-client/compatibility.lock`; completed official hash/source/API/asset/auth feasibility audit.
- **Статус:** новый native Rust client track в `../solaris-client` (NativeClient). Source/provenance audit, не complete client/C02 gameplay/online auth/performance acceptance; honest native Loader approval, own-app provider access и pre-release legal review отдельно open.

### P25 — Native audit: filesystem order/limited codecs не full received-session client

- **Что:** reusable mc-protocol/mc-data/mc-nbt boundaries audited: packet decode presence не full client state machine; mc-data Registry.entries filesystem lexicographic, client runtime IDs must preserve received order. NBT network/named API требует Compound root, не universal text decoder; current ItemStackWireCodec ограничен пятью added/zero removed components, literal text и required registry; metadata/playerinfo/particles inline holders также имеют явные unsupported cases.
- **Зачем:** не переносить SessionRegistry/world authority/server Brain в multiplayer client и не объявлять minimal server fallback full client content. Cached ZIP не activated bundle; wire3 native platform не spoof Fabric/capabilities.
- **Где:** `../solaris-client/docs/COMPATIBILITY_AUDIT.md` (dependency/API/Loader boundary), `../solaris-client/compatibility.lock`.
- **Доказательство:** official verified bytecode/descriptor audit: Play141clientbound/69serverbound, Configuration20/10, Login6/5; lock pins current source hashes, dirty sibling не production dependency.4750current asset objects verify digest/size,142languageobjects differ from cached index despite same size/id. Public receipts names/sizes/digests only; original Mojang bytes ignored, CDN availability не redistribution license.
- **Статус:** audit evidence, not client gameplay/window/FPS/auth gate. Own application registration/redirect/audience + provider-approved access + interactive entitled account missing; no foreign appID/secrets/tokens borrowed, no online auth attempted. Offline profile не закрывает online gate.
- **CompatibilityAudit current-chain clarification:** descriptor SHA1 `ee044d53f31fdcbfa31be3684ad321d44c81c3a5`, current index30 `2c17565f9cd7dbb95294e71fe258d65d6a8f0360`;4608 verified read-only symlink imports+142 downloads89620564B, player caches untouched. Paths `../solaris-client/.analysis/evidence/C00/asset-index-current.json`, `../solaris-client/fixtures/content/source-inventory.json` (hash inventory only), `../solaris-client/.analysis/upstream/native-platform-request.json` (honest versioned negotiation request, не approval). Authored PNG/model/WAV/OGG fixtures MIT/Apache, не Mojang originals.

### P26 — Native automation lease +connection/dimension fence на owner dispatch

- **Что:** Request validates atomic live automation lease +expected connection/dimension epochs непосредственно перед event-owner dispatch. Lease captures dimension, защищая queued actions до watchdog reaction.
- **Зачем:** network dimension transition может race queued mutation; permission/lease admission однажды на enqueue недостаточна.
- **Где:** `../solaris-client/src/api.rs` (`Request`), `../solaris-client/src/lib.rs` (owner fence), `../solaris-client/src/automation/control.rs`.
- **Доказательство:** IRC `NativeClient`, fields authored/two owning implementations coordinated; verified paths.
- **Статус:** contract implementation, runtime verification pending, не accepted behavior.

### P27 — Debug startup: optimize JIT compiler packages, не readiness timeout

- **Что:** two896902byte Wasm components sequential debugCranelift/regalloc2 compile~46s; dev package overrides opt-level3 для cranelift-codegen/regalloc2 уменьшают actual readiness52.513→20.754s, percomponent22.577/23.187→4.614/4.802s.
- **Зачем:** unchanged60s wasm-precommit readiness нужен margin, не weakened gate. Solaris unoptimized/debuggable, assertions/guest settings/parallel_compilation(false) unchanged, no cache/new executor.
- **Где:** `Cargo.toml` devpackage profile, `crates/mc-server/src/main.rs`812–852/log behavior; `tools/harness/__main__.py`297.
- **Доказательство:** `.analysis/codex-logs/startup-gate/report.json`, eventreadiness+realTCP/samecomponentbytes/temporary compileprobe removed; captured unoptimized UB precondition/regalloc hotspots. Pluginhost140tests/strict3crateClippy pass; independent narrow review nofindings.
- **Статус:** measured startup fix integrated Main, hostload different but large compile/readiness effect verified. Manual logs always latest.log/debug.log; stdout mirrors only SOLARIS_HARNESS_LOG_STDOUT=1 (harnesssets). Empty manual stdout **не** sixteen-minute startup proof.

### P28 — Vanilla dropped-item pickup delay10ticks, не inherited4

- **Что:** production4tick delay restored10; deterministic regression refusesages0..9,accepts10; persisted remaining delay после2ticks8,не2.
- **Зачем:** core-client detected mined drop credited/despawned~159ms до actualclient render. Preexisting July19constant не recentintegration race; transient realentity visibility matters.
- **Где:** `crates/mc-net/src/play/session/pickups.rs`35–36, `crates/mc-net/src/play/session/tests.rs` (`spawned_item_cannot_be_claimed_until_vanilla_tenth_tick`).
- **Доказательство:** `.analysis/codex-logs/startup-gate/report.json`; vanilla `/home/kaiserroman/research/client-render-research/decompiled/client-26.1.2-src/net/minecraft/world/level/block/Block.java`405–406 calls defaultdelay, `.../world/entity/item/ItemEntity.java`397–398 sets10. Regression failedage4 beforefix, itemfilter111pass/clocktest/strictClippy afterfix; unchanged core-client afterpickupfix passed266.1s, screenshot inspected worker.
- **Статус:** verified parity correction +realclient gate, no assertion weaken/retries. OldINFO-only core-client logs не доказывают lifecycle timing; production debug trace timestamps extracted beforelateroverwrite, rawsummary provenance retained.

### P29 — Main-owned staged-snapshot rebase: per-file merge, не worker reset

- **Что:** Main rebased solaris-settle staged snapshot onto main using merge-file(base=settleindex,ours=settleworking,theirs=main)+update-index--cacheinfo,131files/3trivialconflicts; same method reconciles formatdrift TickTail.
- **Зачем:** preserve worker implementation/unrelated dirtychanges при mainintegration, не blindlyapply patch или discard changes.
- **Где:** Main integration authority, `AGENTS.md` dirty-tree boundaries.
- **Доказательство:** `.analysis/codex-logs/cp049-rebase-log.txt`, IRC Main2026-10-01.
- **Статус:** completed Main-owned rebase; **не** разрешение worker staging/reset/rebase. Final behavioral gates remain scope-specific.
- **Lead coordination после rebase:** preserved21unstaged3wayhunks, conflicts session/villager imports/worldgen float_roundtrip, pluginrepo unchanged. Core owners reread after interveningchange/EntityIdMapSet migration. TickRepair now owns movementpublication/refresh/fence; no duplicate patchapply, no child midflight gates, lead owns workspacecheck.

### P30 — Loader MCP catalog refresh47, native preview13 не parity

- **Что:** source-derived Loader catalog47tools/47exact native-gap rows включает itemcomponent read+native EditBox set; request_loader_view уже accepted45baseline. Native preview13tools, новые receivedcomponent/source/slot/container и widget focus/responder semantics explicitly unsupported.
- **Зачем:** concurrent Loader update не должен стать stale native acceptance promise; declarations export не runtime invocation/Java→native platform approval.
- **Где:** `../solaris-client/fixtures/mcp/loader-catalog.json`, unchangedbaseline `../solaris-client/.analysis/evidence/C00/loader-catalog-baseline.json`, rawexport `../solaris-client/.analysis/evidence/C02/loader-catalog-export.json`.
- **Доказательство:** IRC `NativeClient`; actual JVM definitions()/toJson() Gson readonly compile/export exit0, ClientMcpTools29940B SHA256 `fd15166a59c62769b4ecc9473c622b9c5ee03ca55ddc18acdfb38c0bb0f180ec`; no runtime tools invoked.
- **Статус:** catalog provenance/gap tracking refreshed, Loader wire3/schema2 Java-only, not honestnative platform/client parity acceptance.

### P31 — Native client C00–C02 commissioned slices accepted, C03 ready

- **Что:** commissioned §25.1 C00–C02 accepted с independent readonlyreviews; sole `LONG_TERM_PLAN` cursorC03ready. C02 actualofficial26.1.2+Solaris offlineSURVIVAL movement/mine/pickup/hotbar/place, authoritative inventory/on-disk playerdata, ownwgpu/nativeX11+GPU frames. Solaris81chunks; isolated approvednative PCM two separate episodes/module536870913 reused/unloaded, physicalsink untouched.
- **Зачем:** complete vertical slice честно отличается от fullproduct/C03–C10/onlineauth/Loader/WASM/service parity. Isolated PCM **не hardware audibility**. Acceptance includes earlier fullsurvival/audio +targeted corrected-finalbinary proofs, **не повторение entire survival/audio onfinalbinary**.
- **Где:** `../solaris-client/LONG_TERM_PLAN.md`, `../solaris-client/COMPATIBILITY_MATRIX.md`; `.analysis/evidence/C00`,C01,C02 owningacceptance.
- **Доказательство:** `../solaris-client/.analysis/evidence/C00/acceptance-receipt.json`, `../solaris-client/.analysis/evidence/C01/acceptance-receipt.json`, `../solaris-client/.analysis/evidence/C02/acceptance-receipt.json`, `../solaris-client/.analysis/evidence/C02/independent-review.json` correct0.96/nofindings; finalhash `4393470821fa4010fb6680c878e1cd5f8e81de6ce17130d198058e0072299eac`,9tests/buildfmt pass, native same-server hotbar/look/frame and realConfigurationEOF originalerror/inputlease release.
- **Механизмы исправлений:** entityID shadow packetdiscriminant → packetID54(PosRot) correctly consumes оба rotationbytes передbool; wholeworldremesh freeze → chunk-local cached meshes/boundaryneighbors/explicitvertexbound (mineack13663→1632ms, not performancegate). Unsupported snowhit sound no guessedstonefallback. Outwardneighbor received sky/blocklight signedextra-layer, lightpacket invalidatesown+adjacentmeshes/missingunknown. Terminalerror/cancel owned independently of boundedpacketqueue, drainthenoriginalerror exactlyonce, explicitDisconnectedfail.
- **Статус:** C00–C02 **accepted**, not full Java-client/Solaris-native/WASM/singleplayer/services. C03–C10 mandatoryunaccepted,13nativepreview/47sourcecatalog gaps notMCPfullrun. HonestnativeLoader wire3/schema2platform approval/ownproviderapp/entitlement/legal gates stillopen, no Fabricspoof.

### P32 — Release-only vehicle mutation omission и debug-only panic fixture

- **Что:** `RegionalOwnerCoordinator::index_vehicle_link` вставлял forward/reverse HashMap edges **внутри debug_assert!**; release исключал обе mutations, restore терял vehicle links, snapshot/index mismatch → InvalidMutation → mc-net fatal. Fix выполняет inserts до assertions.
- **Зачем:** assertions не должны владеть sideeffects; debugpass здесь falsegreen для реального releasevehicletransfer. Отдельный `prepare_worker_panic_publishes_failure_and_releases_claim` опирался на i32MAXneighbour overflow: debugpanic, releasewrap/Absent/DrainedNoPacket, а не гарантированный panic-path.
- **Где:** `crates/mc-entity/src/regional/coordinator.rs` (`index_vehicle_link`/`apply_kinematics_if_current_inner`); `crates/mc-net/src/play/chunk_stream/prepare.rs`, panicfixture `chunk_stream/tests.rs`.
- **Доказательство:** `.analysis/codex-logs/release-tests/main.log` два vehicletransferInvalidMutation failures; `.analysis/codex-logs/release-tests/main-net.log` vehiclecrossing fatal и panicstream DrainedNoPacket. IRC `ReleaseBugs` rootcause/fix; originalreceipts сохранены, не пересняты.
- **Статус:** fix+explicit panickingChunkGenerator atnormalcoordinate, assert injectederror+claimrelease; **postfix validation pending** в isolatedrelbugs target. Expected `.analysis/codex-logs/release-bugs/validation.json` ещё не получен, releasepass не заявлен.

### P33 — Small generic integration authority vs durability/per-tick boundary

- **Что:** Main authority, переданная lead `SettlementsCP049`: tinygeneric constructor/one-linehook/Optionfield/moduledecl hunks **вне holds** не требуют отдельных Mainapproval, но перечисляются lead. Mainapproval остаётся для heldfiles, **нового durability/journalparticipant**, нового perTickwork/vanilla-visible behavior.
- **Зачем:** remove approval bottleneck без arbitrary newowner/lifetime/behavior architecture. Default optimistic conditionalcommit+recovery; no holds/leases/freezes/worldlocks acrossIO, no diskIO ownerthreads, zero-cost unused hooks, genericnames.
- **Где:** командная integration boundary; cachedclassification `crates/mc-entity/src/runtime.rs`, RegionalChunkMask `regional/tick.rs`/ownerphysics; source-worker актуальные holds отдельно от persistent contract.
- **Доказательство:** IRC `SettlementsCP049` MainAUTHORITY2026-10-02: Physics2 landed, classificationhold released; NativePhysicalProjectiles extends **existing exactcache** by vanillaAbstractArrowfamily, не bypass/newphysicscopy.
- **Статус:** active coordination rule. Goal/pose/pathing, chunk_pipeline/light holds тогда сохранялись; историческая запись **не разрешение редактировать их позднее без fresh owner confirmation**.

### P34 — ReleaseBugs final: реальные vehicle restore/migration и explicit panic propagation passed

- **Что:** two-filecut P32 завершён: production forward/reversevehicle-index insertsoutside debugassert, existingvehicle regressions unchanged; deterministic panickingChunkGenerator normal(0,0) вместо overflowfixture.
- **Зачем:** correctreleaseauthority restore+atomiccrosslane motion, не suppression EntityOwnerFailureStatefatal. Panicfixture доказывает **blocking-generator panic propagation**, requestedcoordinate error/incomplete scheduler/claimreacquisition, **не прежний outerasync JoinError overflow fallback**.
- **Где:** `crates/mc-entity/src/regional/coordinator.rs`, `crates/mc-net/src/play/chunk_stream/tests.rs`; `/home/kaiserroman/solaris-relbugs/.analysis/codex-logs/release-bugs/report.json`, `validation.json` там же.
- **Доказательство:** release2vehicletransfer+1authoritativepassengercrossing+1preparepanic allpass; debug650entity/7ignored+2078net/6ignored, strictall-targetClippy/fmtclean. Actual**non-test release RegionalOwnerRuntime** restoresgroup/movesleader/shutdown boat(128.5,64,.5)/passenger(128,64,.5)+intactlink. IndependentReleaseFixReviewcorrect/no findings; temporaryexample/runnerremoved.
- **Validation caveat:** initial6guestartifactmisses caused globalCARGO_TARGET_DIR writing childSDKoutside `sdk/rust/target` expectedbyfixtures; unsetoverride/rerunonlynetfull-libpasses, no unrelatedsource/testfix. This environmentalissue не vehiclecause.
- **Статус:** scopedreleasefixPASS, supersedes P32pending, не projectwide/canonicaltickbudget claim. `/home/kaiserroman/solaris-relbugs/.analysis/codex-logs/release-bugs/release-bugs.patch` SHA256 `6b8a490de92b697b314fd2bb5212211a2804cf60445e4f4a8d39ec9faa9c96e4`.

### P35 — Native profile-texture trust: authlib7.0.63 service keys, не embedded legacy key

- **Что:** actual authlib7.0.63 secure skin/property verification fetches productionHTTPS `https://api.minecraftservices.com/publickeys`, uses `profilePropertyKeys` RSA/X509DER, verifies SHA1withRSA over property **VALUE string**, не старый embedded key и не reserializeddecodedJSON.
- **Зачем:** nativeprofile-texture trust обязан повторять verified target semantics; publickeyfetch **отдельно** от ownMicrosoftapp/entitledaccount onlineauthentication blocker, не оправдание guessed/cachedlegacy trust.
- **Где:** `../solaris-client/.analysis/entity-models/skin-trust.javap`, `services-keys.javap`, `auth-service-endpoints.javap` в той же директории.
- **Доказательство:** IRC `NativeClient`, actualnice19javap-p-v verifiedauthlib7.0.63/0.52s; dumps содержат RSA/X509EncodedKeySpec/SHA1withRSA/profilePropertyKeys/PRODenvironment и concat `/publickeys`, servicesbaseHTTPS authority.
- **Статус:** source-semanticfact, **не nativeHTTP/keyrotation/onlineauth/runtime acceptance**. Providerauthorization/account/legal gates изP24/P31 остаютсяopen.

### P36 — Native26.1.2 `set_time` = gameTime + registry clocks, не legacy dayTime

- **Что:** Playpacket113 `minecraft:set_time` carries gameTime:i64 +map<Holder<WorldClock>,ClockNetworkState>, **не legacydayTime/bool**. WorldClock holder rawregistryVAR_INT без+1; ClockNetworkState totalTicksVAR_LONG+partialFLOAT+rateFLOAT (ownerjavapverified).
- **Зачем:** hardcoded24kday renderer не target26.1authority. NativeC03 evaluator должен потреблять serverworld_clock/timeline registries+tags и currentnetworkclockstate, не infer из старого packetshape.
- **Где:** `../solaris-client/.analysis/evidence/C00/packet-ids.json`; pinnedsource `.analysis/client-automation/versions/26.1.2/client.jar`, class `net.minecraft.network.protocol.game.ClientboundSetTimePacket`.
- **Доказательство:** `artifact://3702:955-1041` actualjavap-c-pfullclass LONG+map WorldClock.STREAM_CODEC/ClockNetworkState.STREAM_CODEC; packetcatalog113verified. IRC `NativeClient.EnvironmentLighting` separateclock/statejavap, sourceartifact пока не on-diskdump; jarpathexists для воспроизведения.
- **Статус:** verifiedsource/wirecontractfact, **nativeC03 runtime/render acceptance pending**, не принятие entireenvironment/daynight evaluator.

### P37 — Native freeze semantics: player и mount subtree продолжают entity ticks

- **Что:** target26.1.2 `TickRateManager.isEntityFrozen = !runsNormally && !(entity instanceof Player) && entity.countPlayerPassengers() ≤ 0`. `ClientLevel` применяет это к root/nonpassenger entities, recursively rideTicks players независимо от freeze.
- **Зачем:** frozen World gameTime не замораживает local player и root mount с player в passenger subtree: physics/spin/dash/seat продолжаются; Camel pose elapsed отдельно следует World time. Blanket native pause всех entity animation/physics нарушает target semantics.
- **Где:** `../solaris-client/.analysis/player-research/TickRateManager.javap`144–158, `ClientLevel.javap`2711–2729/628–675.
- **Доказательство:** IRC `NativeClient`/PlayerMotion fresh actual javap on pinned source; files verified, displayed TickRateManager eligibility matches predicate.
- **Статус:** verified source rule, **не native/GPU freeze/vehicle runtime proof**.

### P38 — Native skin trust: exact textures host, supported image sizes и property bytes

- **Что:** authlib7.0.63 `TextureUrlChecker` допускает **только exact lowercase `textures.minecraft.net`** с http/https, не subdomains/прочие minecraft.net hosts. Profile texture signature — SHA1withRSA по exact property.value UTF8; сам этот verifier **не проверяет recipient UUID**. Target26.1.2 `SkinTextureDownloader` принимает только64×32/64×64; legacy mirror использует `NativeImage.copyRect` destination offsets.
- **Зачем:** native trust/image conversion должны следовать pinned source, не guessed broader hostname allowlist, иной signature payload или неверные mirrored coordinates. Это refinement P35, не online provider permission.
- **Где:** `../solaris-client/.analysis/profile-trust/texture-source.javap`, `url-source.javap`.
- **Доказательство:** IRC `NativeClient.ProfileTrust`, ignored actual source receipts; files verified.
- **Статус:** source semantics, **native network/decode/render acceptance не заявлена**.

### P39 — Native signed chat: exact certificate/message bytes и sticky source chain

- **Что:** remote player certificate signs `UUID(16) + expiry BEi64 + X509DER`, SHA1withRSA/service `playerCertificateKeys`. Chat message SHA256withRSA signs ordered bytes: `BEi32(1), senderUUID, sessionUUID, BEi32(index), BEi64(salt), BEi64(floor(timestampMillis/1000)), BEi32(UTF8 content length), content, BEi32(lastSeenCount), raw256-byte signatures`.
- **Зачем:** UTF8 byte length, seconds truncation, integer endianness, exact signature bytes/order являются protocol authority, не JSON/text approximation. Не смешивать profilePropertyKeys (P35/P38) с playerCertificateKeys.
- **Механизм:** `PlayerInfo` validator использует source8h key-expiry grace; message client expiry7min. Sticky chain rejects nonidentical record с index≤lastindex; **same last record permitted**, index gaps okay. Нельзя подменять source chain strictcontiguous index или blanket duplicate rejection.
- **Где:** `../solaris-client/.analysis/profile-trust/{chat-source,chat-session-source,chat-trust-source}.javap`.
- **Доказательство:** IRC `NativeClient.ProfileTrust`, actual ignored pinned26.1.2/authlib7.0.63 javap receipts; files verified.
- **Статус:** source/wire trust contract, **не native signed-chat runtime/online-auth acceptance**.

### P40 — Native inventory predicates: initialized source item facts и fourteen consumer differential

- **Что:** actual26.1.2/Java25 source `Builtin.DATA_COMPONENT_INITIALIZERS.build(VanillaRegistries.createLookup()).pending.apply` экспортировал **1506 items /16 BannerItem /17 BundleItem /21 PotionBrewing DEFAULT_FLAGS ingredients /5 Item.craftingRemainder**. Read-only item facts source не подменяется uninitialized builtin defaults.
- **Predicate contract:** PresenceAnyValue wirebool должен сохраниться; `custom_data {}` остаётся empty record, не SNBT string. Attribute component value — actual LIST с entry key `type`, predicate key `attribute`; explosion predicate/component keys — `has_trail`/`has_twinkle`. Written-page predicate CODEC — direct Component, equality с `Filterable.raw` после source normalization (tag `type` removed).
- **Зачем:** wire presence, canonical field shapes и initialized data components влияют на real DataComponentMatchers consumer results; JSON/CRC32C/hash approximation не interchangeable с source.
- **Где:** `../solaris-client/.analysis/inventory-contract/item-properties.json`, SHA256 тогдашнего export `78c654e7ab37ee832d7ab826f39885978324c87a1fcf91b00bf0c1cf0a4ddeb5` (**superseded extended export в P43**); native `src/inventory/{predicates,components,nbt,snbt}.rs`, `src/inventory.rs`, `src/ui/hash.rs` относительно client root.
- **Доказательство:** actual source `InventoryPredicateOracle.java`, `predicate-cases.json`, `predicate-goldens.json`, `predicate-{java,native}-receipt.txt`, `predicate-smoke.rs` в той же `.analysis/inventory-contract/`; native receipt **NATIVE_PREDICATE_CONSUMERS14 FAILURES0**. Harness derived inventory/menu codec adapters, scope14 actual JSON+wire+HashOps+`DataComponentMatchers.test` comparisons.
- **Статус:** scoped source/consumer differential proof, **не full inventory/native integration/UI acceptance**; parent NativeClient владеет final integration.

### P41 — Native entity physical kernels: exact Shulker/Piston source differential, не C04

- **Что:** Shulker `getProgressAabb` делает `physicalPeek(double) × scale(double)`, **не f32 product**; native correction в `src/entities/physical.rs::bounds` сохраняет это. Warden pose13/14 height1, windcharge bboxY −0.15f32; `Metadata::int` принимает Direction. BlockEntity lid progress linear source и shared render/physics (`blocks.rs`, `presentation.rs`).
- **Зачем:** small-scale/attachment/animation states меняют реальные bounds/collision; fixed-source classification предпочтительнее guessed renderer/name flags. ExportModels749rig roots/157types берёт `fire_immune` из actual `EntityType.fireImmune()`; reflected `modifyPassengerFluidInteractionBox` owners AbstractBoat20/Entity137.
- **Где:** `../solaris-client/src/entities/{physical,metadata,blocks,presentation}.rs`; `.analysis/entity-models/{PhysicalOracle.java,physical-oracle.json,physical_smoke.rs,physical-kernel-receipt.json,models.json,provenance.json}` относительно client root.
- **Доказательство:** final durable receipt **passed_scoped_kernels_only**, Java/Rust exit0, **236 Java rows /19 actual-source boundary scenarios /120 Shulker comparisons** (6faces×5peek×4scales0.37/1/2.7/3, tolerance1e−12) и **96 Piston moved-STONE collision-union comparisons** (6faces/ext-retract/source-nonsource/4progress, exact coordinate-partition union). Supporting12 head shapes/8 dimension rows **не compared**. Final cacheSHA256 `6676b2239ca570e8d4f8179ee3615f76a8f66ab16c58e9705756c96a849db51c` supersedes earlier source-only b6bb… cache/224rows/18boundaries report.
- **Ограничения:** modules included by `#[path]`, fixture Entity/Store/Content и unclimbable collision adapter **не whole-client integration**. Piston oracle moved stone, **не retracting moved-piston base**. Receipt pins commands/source fingerprints; display boundaries first state/duration/retarget from sampled partial progress тоже scoped.
- **Статус:** scoped physical consumer proof, **NO StageC04/nativeGPU/whole-client build/render acceptance**; major renderer families ещё в работе.

### P42 — Native motion contact: conditional boat fluid box, step effects и armor-only freeze

- **Что:** source `Entity.getFluidInteractionBox` deflates body0.001 и вызывает **immediate vehicle** `modifyPassengerFluidInteractionBox`. AbstractBoat underwater оставляет box unchanged; иначе NULL **только если boat.maxY ≥ passenger.maxY**, в другом случае minY=max(passenger.minY,boat.maxY). **Не blanket boat-wet=false**.
- **Механизм:** source StepBasedCollector extinguish min0 сохраняет negative fire−20; fire+lava+water **same step0**, next-step lava300. Armor freeze проверяет equipment **[2,7)** (humanoid armor+body), **saddle excluded**. `fluid_contact_scan(rawBounds,clippedScan)` depth использует original body.minY.
- **Зачем:** mount shielding зависит от actual box/underwater state, а fire/water/lava aggregation — от step; categorical vehicle-dry или unconditional fire-zero меняет source behavior.
- **Где:** `../solaris-client/src/player/vehicle.rs` (`source_in_water`, `passenger_fluid_box`, `mount_can_freeze`), `movement.rs` (`fluid_contact_scan`), `collision.rs` (`apply_inside_effects`, `apply_effect_step`, `collect_inside`, `swept_inside`) в той же player directory. Source `.analysis/player-research/{Entity,AbstractBoat,EntityFluidInteraction}.javap` (11141/2203/97–100).
- **Доказательство:** durable `.analysis/player-research/motion-completion/{java-boat-fluid.txt,contact-smoke-output.txt,java-collector.txt}` относительно client root: actual Java25/native helper одинаковые EXPOSED0.6:1.7990000000000002, UNDERWATER0.201:1.7990000000000002, ENCLOSEDnull. Native receipt также scoped tripwire swept/above refusal, endportal vertical bounds и strict powder-snow fallDistance boundary. Java collector receipt−20/0/300; reflected owners AbstractBoat20/Entity137. Java oracle throwaway sources удалены после smoke, receipts retained.
- **Статус:** scoped source/helper/collector proof, **не full native-client/player/vehicle acceptance**.

### P43 — Native inventory completion: source name owners, reversed registry IDs и scoped actions

- **Что:** initialized actual26.1.2/Java25 item export дополнен `description_id` из `getDescriptionId` и `name_owner` из reflected `getName(ItemStack)`. **Все seven source owners**: AirItem, CompassItem, Item, PlayerHeadItem, PotionItem, ShieldItem, TippedArrowItem; UI не должен считать potion/playerhead единственными special name owners.
- **Где:** `../solaris-client/.analysis/inventory-contract/{InventoryItemProperties.java,item-properties.json,item-properties-receipt.txt}`; extended JSON SHA256 **5967c86b6c70ac978cd4ee6dccf3599a6b9bc1afb627cdae2d36bfea6078f215** supersedes P40 fingerprint. Counts unchanged:1506 items/16 banners/17 bundles/21 brewing ingredients/5 remainders.
- **Доказательство components/creative:** final `golden-smoke-final-receipt.txt` **SOURCE_COMPONENT_GOLDENS110 FAILURES0**, `predicate-smoke-final-receipt.txt` **NATIVE_PREDICATE_CONSUMERS14 FAILURES0**, `creative-smoke-final-receipt.txt` **CREATIVE_SOURCE_PRESETS7931 FAILURES0 REVERSED_DYNAMIC_HOLDER_IDS**; independent actual Java `ItemStack.STREAM` decode в `creative-java-oracle.txt` **7931 FAILURES0**, oracle `CreativeWireOracle.java` и native wire `creative-native-wire.json`. Все paths относительно той же inventory-contract directory.
- **Зачем:** reversed dynamic holder IDs проверяют registry lookup, не удобный canonical ordinal; source name owner не выводится из item-name heuristics.
- **Scoped actions:** `action-smoke{.rs,-receipt.txt}` — actual CB18/20 execute source-fixture PASS pickup/state fence/atomic rollback/**server snapshots**/creative palette+throw+trash/feature gate/anvil rename+XP/template. Pinned26.1.2 **SB creative packetID56**: fixedi16 slot−1 для drop, затем updated target slot38; trash очищает **46 inventorymenu slots0..45**, каждый тем же SB56 с ordinary **full optional ItemStack**, не hashed-stack substitute. Это **не live-server connection/native-window receipt**.
- **Статус:** current-source isolated rustc/Java proofs; **не full native C03/C04 acceptance**, parent NativeClient владеет real renderer/server verification.

### P44 — Native world border: received lerp duration — source ticks, не legacy milliseconds

- **Что:** pinned Minecraft26.1.2 `WorldBorder.MovingBorderExtent` interpolation **tick-based**, **не wall-clock milliseconds**. Constructor ставит `lerpProgress=duration`; source-world `update` уменьшает remaining progress на1, переносит current size в previous и вычисляет fraction `(duration−progress)/duration`.
- **Wire contract:** `ClientboundInitializeBorderPacket` читает/пишет **VarLong remaining `getLerpTime()` ticks**, не presumed durationms/`Instant`. Source defaults absoluteMaxSize **29999984**, initial size **59999968**.
- **Зачем:** legacy protocol documentation не authority для новой actual source version; milliseconds inference смещает border относительно source ticks.
- **Где/доказательство:** `../solaris-client/.analysis/evidence/C04/world-heightmap-border-source.javap`; constructor1685–1726, calculate1848–1873, update1925–1958, InitializeBorderPacket1986–2107, defaults660–699. Actual pinned bytecode подтверждает duration/progress и tick decrement; IRC NativeClient source receipt.
- **Статус:** source/wire contract, **не wall-clock run/native window/world-border gameplay acceptance**.

### P45 — Native WORLD_SURFACE: padded received map, source priming и omitted replacement retention

- **Что:** packed WORLD_SURFACE decoder/update использует received **padded** map для height lookup (в том числе beacon). Malformed array length **prime from received blocks**, не blanket packet rejection; **omitted replacement maps сохраняют existing values**.
- **Зачем:** Source допускает и обрабатывает эти состояния; replacing source heightmap на guessed top/block map либо отказ на malformed длине меняет consumer behavior.
- **Где:** `../solaris-client/src/protocol/world.rs`, **`src/protocol/world/heightmap.rs`** относительно client root; durable `.analysis/evidence/C04/heightmap-source-proof.json` (source fingerprints), `heightmap-oracle.json`, `heightmap-java-{compile,run}.json`, `heightmap-scoped-tests.json`.
- **Доказательство:** original Minecraft26.1.2 Java `Heightmap` + real `ProtoChunk/LevelChunkSection/palette` создаёт actual section wire bytes/transitions; actual Rust World/Reader/JavaRandom source compiled directly. **12 scenarios**, heights **16/384/512/4064**, **2112 block updates**, **14400 height checks**, **0 mismatches**, **3 existing World tests PASS**. Scope valid/malformed/omitted maps, negative coordinates, top/bottom air/water/non-air edits, immutable snapshots, omitted-map replacement retention; malformed length1 primes expected22/37/43/52 longs.
- **Статус:** scoped actual-World proof; canonical Session packet integration/render/Content loading/whole-client compilation/native window и **C03/C04 acceptance не proved**. Probe scaffolds removed, durable receipts retained.

### P46 — Native renderer source correction: terrain sampler и EYES cull/blend

- **Что:** earlier `TextureAtlas NEAREST` claim **не покрывает terrain**. Actual `OriginalLevelRenderer.chunkLayerSampler`: **CLAMP_TO_EDGE + LINEAR min/mag**, mipLINEAR, default NONE filtering gives anisotropy1; entity/item имеют separate own NEAREST.
- **EYES contract:** previous root EYES no-cull summary неверен: **cull=true** (default builder, no `withCull(false)`). Alpha OVER **SrcAlpha/OneMinusSrcAlpha color + One/OneMinusSrcAlpha alpha**; **LEQUAL/no depth write/no cutoff**; flags **EMISSIVE/NO_OVERLAY/NO_CARDINAL_LIGHTING**.
- **Зачем:** atlas-own sampler state не interchangeable с render-pass supplied sampler; inferred no-cull/light/cutoff меняет source pipeline.
- **Где/доказательство:** `../solaris-client/.analysis/entity-render-special/pipelines.json`, source EYES entry `eyes[minecraft:textures/entity/sourceprobe.png]` inspected через bounded projection: cull/blend/depth/defines exact. Actual `OriginalLevelRenderer` bytecode `artifact://4670:4089-4137` и `RenderPipelines` `artifact://3675:1200-1238` (NativeClient source receipt); first artifact confirms supplied chunk sampler, not guessed atlas filter.
- **Статус:** source-state correction, **GPU execution/native-window/C03/C04 acceptance пока не proved**.

### W15 — W5 rocky peaks: provisional presentation thresholds, не derived treeline

- **Что:** altitude≥1500m/slope≥0.1 — provisional presentation thresholds; spec§10.2 и Earth climate задают лишь lapse≈6°C/km, **не treeline/alpine belt или slope cutoff**.
- **Зачем:** не выдавать presentation heuristic за физически/source-derived ecological boundary.
- **Где:** `../solaris-w5/docs/evidence/worldgen-w5.md`, Rocky-peak threshold provenance; Earth `climate.rs`14/579, `region_geology.rs`436–455.
- **Доказательство:** IRC WorldgenW5b: recovered compile PASS (`recovery-check.log`); fresh128-page acceptance probe pending.
- **Статус:** threshold provenance correction; **не W5 acceptance/treeline derivation proof**.

### C16 — Loader recovery: six scoped suites PASS, не graphical acceptance

- **Что:** исправлены missing `Set`/`LinkedHashSet` imports; migrated ruby/sapphire fixture archives rebuilt. Generic bridge unchanged.
- **Где:** Loader `LoaderViewScreen.java`, `README.md`; settle fixtures `plugin.toml` + `client/rich-content.zip`.
- **Доказательство:** IRC SettleLead2; `/home/kaiserroman/solaris-settle/.analysis/codex-logs/settle/cp049-loader-recovery/{receipt.json,scoped-gradle-tests-verified-final.log}`: six scoped Gradle suites, **443 tests /0 failures**; final receipt reuses two initially successful suites.
- **Статус:** scoped recovery proof; **graphical acceptance ещё не proved**.

### W16 — W5 recovered review closure: eight findings closed, acceptance ещё open

- **Что:** все eight prior W14 review findings closed; integrated fixes имеют exact passing behavioral receipts, включая **WAL spawn-remove allocator highwater** и **evicted inhabited-tail failed-load retention**; remaining review-blockers отсутствуют.
- **Где/доказательство:** IRC WorldgenW5b / completed W5ReviewClose (no source changes); `../solaris-w5/.analysis/codex-logs/worldgen-w5/review-closure.json`. Workspace `--all-targets` check PASS в `workspace-check.log`.
- **Статус:** recovered review closure supersedes W14 open review list; strict lint выявил new style lints, fixes pending at report time. **Не W5/runtime/graphical acceptance claim**.

### P47 — Native client owner reprioritization: agent-test P0, затем C05 Loader

- **Что:** owner-directed NativeClient2 priority — **P0 agent-test client**, headless **без Java/Xvfb**, real offscreen GPU + RSS/CPU receipts; **EXACT existing Java `minecraft_*` schemas/semantics**, one implementation и event waits. Затем **C05 Loader codec/generic owned UI**.
- **Зачем:** cheaper harness agent tests и settlements gates. **C03 scaleperf/rest C04 coverage explicitly deferred, не accepted**.
- **Где/доказательство:** IRC NativeClient2, Main user interjection в current NativeClient2 history; priority notice + short-plan direction. ≤6 existing children steered к disjoint MCP groups/GPU; HarnessNative owns core backend contact.
- **Статус:** authoritative reprioritization, **не implementation/GPU/schema-equivalence acceptance**.

### W17 — W5 recovery strict lint PASS, fresh source-fingerprint probe required

- **Что:** strict all-target Clippy **`-D warnings` PASS** для mc-entity/net/server/simulation/test-harness/world/worldgen; fixes — aliases, collapsed conditionals, redundant clone/borrow. Supersedes W16 lint-pending status.
- **Зачем:** Earth source-byte fingerprint изменён этими edits; prior artifacts не заменяют **fresh v4 prepare +128-page production probe**.
- **Доказательство:** IRC WorldgenW5b; `/home/kaiserroman/solaris-w5/.analysis/codex-logs/worldgen-w5/strict-clippy-final.log`, completed mem-run bg45, Finished dev profile optimized,2m28s; earlier `workspace-check.log` PASS.
- **Статус:** lint/compile recovery proof; fresh probe required, **не W5 acceptance**.

### W18 — W5 fresh v4 production coastline probe: 128 pages PASS, graphical ещё open

- **Что:** final v4 actual prepare + real production vanilla-content coastline probe **all128pages PASS**:59rawpages/72rawsites,43acceptedpages/52acceptedsites,**72.22% site acceptance**. Dry spawn **(−7625000,85,−1875000)**; selected coastal footprint **227.976m от ocean**; native persistent record counts **all0**.
- **Зачем:** свежая preparation после W17 source-fingerprint change, **no autorebase/authored island/weakened site predicate**; closes fresh-probe requirement, не graphical gate.
- **Где/доказательство:** IRC WorldgenW5b, completed mem-run bg47; `/home/kaiserroman/solaris-w5/.analysis/codex-logs/worldgen-w5/final-v4/{site-probe-summary.json,selected-village-water.json,prepare.log}`; identity **`0aa17daf73607162632ae6ab9e38ff2b008d1c926ac84bc90a230f654af78a55`**.
- **Статус:** actual production preparation/site proof **before graphical**, не whole W5/client acceptance. Probe source retained ignored `final-v4/w5_site_probe.rs`; throwaway production example removed.

### C17 — Native storage actor stack overflow: boxed order dispatch, не stack-size bump

- **Что:** `Box::pin` на NPC-order dispatch boundary в `storage.rs` исправляет native storage actor stack overflow; **не увеличение thread stack**.
- **Зачем:** gdb+objdump измерили nested debug frames: actor≈1.1MiB + dispatcher≈545KiB + order≈325KiB; RawTask captures только1696B. Generic owner rationale обновлён в `docs/architecture/runtime.md`.
- **Доказательство:** IRC SettleLead2; `/home/kaiserroman/solaris-settle/.analysis/codex-logs/settle/recovery/{native-order-stack-before.json,native-order-stack-debug.log,native-order-stack-after-retry.log}`: exact regression **1 PASS**. Physical-profile caller migration compiles; mc-script scoped **7 PASS**, SDK compile **0 tests**.
- **Статус:** reproduced/scoped fix proof; **full native-order rerun pending**, не whole-product acceptance.

### W19 — W5 final native graphical run: Play/walking proved, village phase failed

- **Что:** actual final native canonical graphical run достиг **Play**, dry-safe stone/full health; two ordinary key walks, max **51.366891m от spawn**,12screenshots. После tp на selected village **(−7124789,54,5000158)** exact client query failed **unloaded block**, native phase **exit1**.
- **Ограничения:** **village/water/edit/death/restart acceptance не claimed**. Native checkpoint13.5–15.0s; shutdown6sdeadline exceeded, final save completed late. Latest user ordered wrap current step → checkpoint **explicitly incomplete**, не accepted W5.
- **Доказательство:** IRC WorldgenW5b, genuine screenshot inspected; `/home/kaiserroman/solaris-w5/.analysis/codex-logs/worldgen-w5/recovery-wrapup.json`; `graphical-9ghlyovr/w5-earth-before/20261002T051347Z-graphical-manifest-Waz40L/{observations.json,automation-driver.txt,server.log,screenshots}`.
- **Integration artifact:** `worldgen-w5-recovery.patch`, SHA256 **`7db92b4ad659cc5ba28232dc44e19583b2ba0740a8c9490bcb118bc1887045a6`**; patch artifact **не integration proof**.

### T31 — TickCritical2 death-RPC root cut: fenced sparse passenger walk, gate still FAIL

- **Что:** full-store owner-prepare snapshot reference scan и removal N-ID allocation заменены **RuntimeEntityIndex-fenced sparse PassengerState archetype walk**; synchronous death/CAS timing сохранён, **не async deferral**.
- **Где:** `../solaris-tickphys3/` runtime.rs/lib.rs + regional owner lane. IRC TickCritical2; `combined-window/summary.json` canonical100k/1200: due-death lifecycle p95/p99 **318/610µs**, owner-prepare snapshot/reference p95 **4/1µs**, owner-apply p95/p99 **28/31µs**. Prejournal library tests **663 entity /2087 net /90 physics PASS**.
- **Gate:** canonical6CPU **p95/p99 84.844/101.346ms FAIL**; old73/84 scope unknown. Main plans identical **A/B/A/B** before claiming regression, не causal comparison сейчас.
- **Journal follow-on:** InsertedBody **42/42 dominant invalidations** теперь independent body-ID record; carrier/missing-state force full. **Final gate after fix ещё не run**.

### T32 — Ordered sleeping-slot lookup preserves the full owner selection

- **Что:** cached signed-ID/slot projection merges with the existing unique ascending ECS feed instead of hashing every sleeping ID. Successful membership changes enqueue IDs; only those IDs are sorted/deduplicated and merged using final authoritative slots. Stable FIFO holes, UUID/motion/section fences, wake timing and full physics fallback remain unchanged.
- **Где:** `crates/mc-entity/src/regional/owner_lane/physics.rs`; serial-reference churn regression in `physics_tests.rs`.
- **Доказательство:** `.analysis/codex-logs/tick7/`: fresh Main snapshot `88808eb7…` passed owner-sleep/regional/runtime gates. Final delta version passed seven sleep shadows, 215 regional tests (four ignored), strict mc-entity/mc-net Clippy and code-health; two independent reviewers found no correctness issue. Baseline/final 100k/25-region/50-client, 200/1200-tick FP captures have 73,576/80,592 owner samples; baseline sleep hash lookup was 8.061% self. Paired actual ECS/cache/fence kernel at 4096 bodies: steady lookup −15.05%, 5% wake/readmit −10.67%, mixed 25% misses/5% churn −12.22%; full-resort prototype was rejected for ~12% churn regression.
- **Ограничение:** CPU4 runs are noncanonical shared-host attribution, not a causal whole-tick comparison. Final tick p95/p99 **185.345/231.009ms still FAIL** the 50/60ms budget. All-miss kernel at 256/4096 bodies was ~1% slower; mutation itself was outside kernel timing, while projection rebuild and old-leg queued-update backlog were included. No universal gain, capacity/budget acceptance or AI/cadence reduction is claimed. Commands, exact measurement source and receipts stay in the owning artifact directory.

### P48 — Native P0 user-requested wrap: catalog scoped proof, implementation UNFINISHED

- **Что:** NativeClient2 P0 **UNFINISHED** на user-requested wrap: canonical Session + headless owner + dispatcher still unintegrated. **C00–C02 acceptance unchanged; C03/C04 deferred; C05 after P0**. Old C02 binary **не new proof**.
- **Где:** `../solaris-client/LONG_TERM_PLAN.md` cursor `P0-agent-test-client` in_progress; `docs/AGENT_TESTING.md` truthful partial-source status.
- **Доказательство:** IRC NativeClient2; `.analysis/native-catalog/catalog-smoke-receipt.json` actual rustc1.95 compile0/run0 для production catalog **47 Java names**, closed args, physical keys, numeric boundaries/no aliases. **Не full-client/GPU/gameplay/RSS/CPU acceptance**.
- **Source cutovers:** RootStore `load_models` → **`activate_models(Arc<Models>, &Content)`**, background parse/preserve generation; InventoryStore `received_menu`/`received_player_slot` **0..47 allocation-free**, no fake unknown auxiliary slot.
- **Process rule:** Main latest owner exempts single-file javap/javac/rustc from mem-run; **heavy cargo/gradle/harness remain mem-run**.

### S14 — Settlement user-requested wrap: native 91 PASS/2 FAIL, CP049/050 NOT ACCEPTED

- **Что:** user explicitly ordered wrap before acceptance. Native NPC suite **91 PASS /2 FAIL**; both physical-profile tests PASS после **test-only `Box::pin` repeated continue futures** (debug test poll1.13MiB caused nested overflow).
- **Remaining failures:** `haul_moves_items_between_canonical_npc_endpoints` — no order payload; `committed_handoff_and_cancel_recovery_preserve_later_native_progress` — `routes.rs:248` None. Не suppress/refixture acceptance.
- **Доказательство:** IRC SettleLead2; `/home/kaiserroman/solaris-settle/.analysis/codex-logs/settle/recovery/{native-orders-after-physical-box.log,native-profile-stack-debug.log}`. `loader-server-startup-debug.log:85–93` доказывает Wasmtime/regalloc2 ещё compiling после bounded180s; original readiness90s failed.
- **Статус:** no increased timeout applied; **no live graphical acceptance**, **CP049/050 NOT ACCEPTED /не READY FOR L2**. C17 full native-order rerun завершён с указанными failures, не green suite.

### S15 — Component recovery: fresh immutable package PASS, runtime/graphical proofs unexecuted

- **Что:** в `../solaris-default-plugins/` restored original synthesized cue SHA256 **`fac0330efd6f3792c7fa580fbee9d898c071a75120704c2a416ced2f37d73578`** (prior11e3… remains archived); Cargo-generated guest lock добавил missing blake3+SDK serde; `content-src/pipeline.json` revision2 — grounded ordinary source declarations iron_ingot/leather/string для existing catalog. README documents cue recovery.
- **Доказательство:** IRC SettleLead2.SettleComponentRecovery; fresh immutable package **native+Loader authoring validation PASS**, **56 recipes**, `/home/kaiserroman/solaris-settle/.analysis/codex-logs/settle/CP049/component-recovery-pack-20261002`; component SHA256 **`f0a17c4d423d720fd7042cb85b0086ab386f7969586285694dcfb1cb0fddfb1c`**.
- **Build receipt:** `fresh-component-build-after-reachability.{json,log}`; memory.peak **8589934592**, Main-authorized exclusive8G/jobs2 после6G OOMs.
- **Статус:** user wrap prevents new selectors: required three runtime proofs и existing CP050 scoped behavior proofs **unexecuted after rebuild**, graphical gates unrun; authoring receipt explicitly **activation not exercised**. **CP049/050 remain unaccepted**.
