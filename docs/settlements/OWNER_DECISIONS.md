# Решения владельца: CP-058 / CP-059 / CP-068 (RULESET DECISION)

Источник: RULESET DECISION совета (Astra + GLM second opinion; владелец правит при желании).
Значения ниже — дословно из решения; реализовывать ровно это. CP058/059/068 разблокированы
на этих значениях. Открытым остаётся только native profile кормления/HP/stamina/collider/seat/impulse.

## CP058 — война

- Объявляет: charter-holder, committed declaration + доставленная причина.
- Grace: 72000 simulation ticks.
- Окна: ≤8, каждое ≤72000, пауза ≥12000, абсолютные [start,end).
- Цели: ≤8 lawful.
- Размер record: ≤4096 UTF-8 bytes.
- Protection: Private/Neutral/Unknown = отказ; после peace/revoke 0 новых damage commits.
- GLM-поправки ПРИМЕНЯЮТСЯ:
  - (a) НЕТ автопобеды удержанием точек — победа ТОЛЬКО по committed peace/surrender с записанным исходом; истечение окон без исхода = стоп эффектов, победителя нет.
  - (b) auto-contribution = 0 (только добровольная оплата реальным inventory-трансфером).
  - (c) победа НЕ даёт отдельного права пролома/salvage.
  - (d) AdmittedConflictContinues: logout НЕ даёт иммунитета в уже допущенном lawful-конфликте текущего окна, но новых offline-целей нет.

## CP059 — снабжение

- Saddle: 3 leather + 1 iron = 1 saddle, crafting_table, 240 ticks.
- Hay: 9 wheat → block 80 ticks; block → 9 wheat 40 ticks.
- Оборот: atomic debit/output, без replay-минтинга.
- НЕТ фикс. цены 10–12 (цена из secured demand).
- НЕТ квоты 4-на-поселение (лимитеры = материалы + worker + station).
- Authorized trade deliveries разрешены.

## CP068 — маунты

- Horse — боевой; donkey/mule — только вьючные, без charge.
- Скорости: индивидуальные native speeds (числа Astra НЕ объявлять ванильными gait-профилями).
- Charge: max +3HP, только копьё/пика, после 40 тиков разгона.
- НЕТ forced dismount на 2 блоках (collision ограничивает движение; dismount только в safe-позицию).
- Lawful transit разрешён (проход ≠ право урона).

## CP068-R2 — каталог

- Значения: OWNER_DECISIONS §1 плагинного репо (`../solaris-default-plugins/docs/settlements/OWNER_DECISIONS.md`) без изменений.
- Spear — явно двуручный.
- Дробный wear накапливается.
- Одна стрела возвращается max один раз.
- Swap/reconnect/deploy никогда не восстанавливают stamina.
- Определить native arrow range limit.

## Charge — принятые числа (дословно)

- Trigger: ≥0.45 blocks/sim-tick, sustained 40 ticks.
- Bonus: линейно 0→3 HP от 0.45→0.60 blocks/tick, cap +3 далее.
- Только spear/pike profile.
- Одно попадание на цель за charge.
- Re-accelerate после контакта.
- Mounted feed/HP/stamina/collider/seat/impulse native profile — всё ещё OPEN.
