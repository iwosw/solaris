# Settlements / Поселения

## Quick start / Быстрый старт
- `/settlement guide` — step-by-step tutorial in chat (also announced on join) / пошаговое обучение в чате, подсказка при входе; `/help settlement` — subcommands / команды.
- `/settlement create <name> [radius]` — found here (radius 1–256, default 48); name: 1–64 ASCII letters/digits/`_`/`-` / основать здесь (радиус по умолчанию 48); имя: латиница, цифры, `_`, `-`, без пробелов.
- `/settlement list` / `/settlement info [name]` — names, owner, anchor, radius, residents / список, владелец, центр, радиус, жители.
- Nameplates and bell/info summaries show jobs and owner names; custom name tags are preserved. Old saves may show an owner's UUID until they next join / подписи и сводки показывают занятость и главу, имена от бирок сохраняются; старый UUID заменяется именем после входа владельца.
- House/work/bell labels appear within 48 blocks after the chunk loads / подписи домов, рабочих мест и колокола видны в 48 блоках после загрузки чанка.
- Generated `Name · profession · activity` plates follow the villager brain and are not saved as name tags / автоматические имя, профессия и занятие следуют поведению жителя и не заменяют сохранённое имя от бирки.
- `/settlement rename <name> <new>` / `remove <name>` — rename or dissolve / переименовать или удалить.
- Stand next to a villager (≤8 blocks): `/settlement bind <name>` / `/settlement unbind` assigns/releases the nearest; optional UUID selects one explicitly / встаньте рядом: привязать/отвязать ближайшего, UUID необязателен.
- `/settlement relocate <source> <household> <destination>` — move a household; housing claims are reacquired / переселить семью, жильё выбирается заново.
- Anyone can found/read; changes require the owner or an operator (relocate: both towns) / создать и читать может любой; менять — владелец или оператор (переезд: права на оба города).

## Settlement menu / Меню поселения

`/settlement` (no args) or `/settlement menu [main|buildings|certify|residents|production|settings]` opens server-driven dialogs for the settlement around you (else the one you own). Screens:
- **Main:** owner, radius, working/idle residents, building counts, storage contents.
- **Buildings:** each certificate's level, job slots, missing requirements for the next level and an Upgrade button. **Certify new building:** choose a type and two opposite corners (`x y z`); stand in a corner block and press **Corner 1/2 = my position**, or enter coordinates. **Height above corners** extends above the higher corner. A failed check returns the form with missing requirements.
- **Residents & jobs:** filled/free slots per building and who works where.
- **Production orders:** status list with Pause/Resume/Cancel. **New order:** choose a crafting table inside a certified workshop, product, quantity slider and resident. The resident crafts from ingredients in their own inventory.
- **Settings:** rename.

Everyone can view; only the owner or an operator can change (re-checked on every click). Labels are English: client locale is not sent to the core.

`/settlement` без аргументов или `/settlement menu [main|buildings|certify|residents|production|settings]` открывает диалоги поселения вокруг вас (иначе вашего). Экраны:
- **Главная:** владелец, радиус, работающие/свободные жители, число построек, содержимое складов.
- **Постройки:** уровень, рабочие места, недостающие условия следующего уровня, кнопка Upgrade. **Certify new building:** тип и два противоположных угла (`x y z`); встаньте в угловой блок и нажмите **Corner 1/2 = my position** либо введите координаты. **Height above corners** добавляет блоки выше верхнего угла. При отказе форма возвращается со списком недостающего.
- **Жители и работа:** занятые/свободные места в зданиях, кто где работает.
- **Заказы производства:** список со статусами и Pause/Resume/Cancel. **New order:** верстак внутри сертифицированной мастерской, продукт, ползунок количества и житель. Житель крафтит из ингредиентов своего инвентаря.
- **Настройки:** переименование.

Смотреть может любой, менять — только владелец или оператор (проверка при каждом клике). Надписи английские: язык клиента до ядра не доходит.

## Villager jobs / Профессии жителей
Adults seek a free workstation during day ticks **2000–8999**, with a walkable path. Unbound villagers retain vanilla workstation acquisition within **48 blocks**. **Bound residents only acquire slots in certified production buildings**; placing a workstation alone no longer creates a settlement job. Free adults automatically fill reachable free slots during their normal allocator pass, including after certification/upgrade. Keep the village loaded.
Взрослый ищет свободное рабочее место днём, тики **2000–8999**, с проходимым путём. Непривязанные жители сохраняют ванильный поиск рабочих блоков в пределах **48 блоков**. **Привязанные жители получают только места в сертифицированных производственных зданиях**; одного рабочего блока недостаточно. Свободные взрослые автоматически занимают доступные места при обычном проходе распределителя, в том числе после сертификации/улучшения. Держите деревню загруженной.
Nitwits never work; traded villagers keep their profession. A villager keeps its workstation while sleeping elsewhere or while the workstation chunk is unloaded, and returns the next workday. A destroyed or stolen workstation releases its claim; an unloaded chunk alone does not.
Бездельники (nitwit) не работают; торговавшие сохраняют профессию. Житель помнит рабочее место, когда спит в другом месте или чанк с рабочим блоком выгружен, и возвращается в следующий рабочий день. Уничтоженный или занятый другим жителем блок освобождает привязку; одна выгрузка чанка — нет.

**Existing worlds:** there is no legacy workstation mode. A bound resident whose remembered workstation is not covered by a certified workshop releases that job claim, but keeps their profession and offers (including untraded offers). The main menu shows **“N workers need a certified workshop — open Buildings → Certify”** for these residents. Certify matching workshops around their existing workstations to let the allocator assign their slots again; no replacement villagers or jobs are created.
**Существующие миры:** отдельного старого режима рабочих блоков нет. Если запомненный рабочий блок привязанного жителя не входит в сертифицированную мастерскую, освобождается только привязка к месту работы; профессия и предложения торговли сохраняются, даже если житель ещё не торговал. Главное меню показывает **«N workers need a certified workshop — open Buildings → Certify»**. Сертифицируйте подходящие мастерские вокруг прежних рабочих блоков, чтобы распределитель снова назначил места; новые жители или фиктивные рабочие места не создаются.


Villagers open **wooden doors**, never iron or copper doors, while following their path. They close doors they opened after passing, including on the way to bed, but wait while another villager occupies or approaches the doorway; player-opened doors are left alone. Progressing routes continue without a forty-tick reset; stalled routes are reconsidered at the next retry, and changed terrain invalidates the route immediately.
Следуя по пути, жители открывают **деревянные двери**, но не железные и не медные. Открытые ими двери закрываются после прохода, в том числе по дороге ко сну; пока другой житель находится в проёме или подходит к нему, дверь остаётся открытой. Двери, открытые игроком, жители не закрывают. Если житель продвигается, путь не сбрасывается каждые сорок тиков; при застревании он пересчитывается при следующей попытке, а изменение местности сразу делает старый путь недействительным.

| Workstation / Блок | Profession / Профессия |
|---|---|
| composter / компостер; lectern / кафедра | Farmer / фермер; Librarian / библиотекарь |
| barrel / бочка; smoker / коптильня | Fisherman / рыбак; Butcher / мясник |
| blast_furnace / плавильная печь; brewing_stand / зельеварка | Armorer / бронник; Cleric / священник |
| cartography_table / стол картографа; fletching_table / стол лучника | Cartographer / картограф; Fletcher / лучник |
| cauldron / котёл; stonecutter / камнерез | Leatherworker / кожевник; Mason / каменщик |
| loom / ткацкий станок; smithing_table / кузнечный стол; grindstone / точило | Shepherd / пастух; Toolsmith / инструментальщик; Weaponsmith / оружейник |

## Certified buildings / Сертифицированные здания

Build freely; certification inspects your actual loaded blocks and never places blocks or charges resources. Certify/list use the settlement containing your position (nearest anchor, then ID when radii overlap); upgrade selects by building ID from anywhere. Mutations require the selected settlement's owner/operator. IDs are printed on certification and listing.
Стройте свободно: сертификация проверяет реальные загруженные блоки, ничего не строит и не списывает ресурсы. Certify/list выбирают поселение, в радиусе которого вы стоите (при пересечении — ближайший центр, затем ID); upgrade выбирает здание по ID из любой точки. Изменения доступны владельцу выбранного поселения/оператору. ID выводится при сертификации и в списке.

- `/settlement building certify <kind> <x1> <y1> <z1> <x2> <y2> <z2>` — certify level 1; coordinates are inclusive, ordered minimum then maximum. Include the floor and roof in the selection. Re-certifying the same kind with bounds containing an existing certificate refreshes/expands it without changing its ID or level; intersecting other certificates is rejected.
- `/settlement building upgrade <id>` — recheck the existing bounds against the next level (maximum 3); failure lists the missing requirements and changes nothing. Build the extra blocks first. To enlarge the selection, re-certify expanded bounds before upgrading.
- `/settlement building list` — IDs, kind, level and bounds in the current settlement.
- `certify` — уровень 1; координаты включительные, сначала минимум, затем максимум. Включайте пол и крышу. Повторная сертификация того же типа с границами, полностью включающими прежние, обновляет/расширяет сертификат, сохраняя ID и уровень. Пересечения с другими сертификатами запрещены.
- `upgrade <id>` — проверяет следующий уровень в прежних границах (максимум 3); при отказе перечисляет недостающие условия и ничего не меняет. Сначала достройте здание; для расширения границ повторите `certify`, затем `upgrade`.
- `list` — ID, тип, уровень и границы зданий текущего поселения.

Kinds / типы: `house`, `storage`, `barracks`, `wall`, `tower`, `gate`, `mine`, `lumber_camp`, or any vanilla profession name from the table above in lowercase (e.g. `farmer`, `toolsmith`, `librarian`) for its workshop / название ванильной профессии строчными буквами для её мастерской.

| Requirement / Условие | Level 1 / Ур. 1 | Level 2 / Ур. 2 | Level 3 / Ур. 3 |
|---|---:|---:|---:|
| Footprint area, all except wall / Площадь, кроме стены | 9 | 16 | 25 |
| Inclusive height, except wall/tower / Высота включительно, кроме стены/башни | 4 | 4 | 4 |
| House beds / Кровати дома | 1 | 2 | 3 |
| Barracks beds / Кровати казармы | 2 | 4 | 6 |
| Storage containers / Контейнеры склада | 1 | 2 | 3 |
| Workshop/mine/camp workstations and job slots / Рабочие блоки и места | 1 | 2 | 3 |
| Wall length × height / Длина × высота стены | 5 × 3 | 9 × 4 | 13 × 5 |
| Tower height / Высота башни | 6 | 9 | 12 |
| Gate doors / Двери ворот | 1 | 2 | 3 |

Ordinary buildings require a complete collision-bearing floor across the selection; mine bounds instead designate the digging zone. Houses/barracks need real head+foot bed pairs with baby headroom, a door, a complete roof on the top plane and a closed perimeter between floor and roof (doors/windows with collision count). Towers require the same roof/perimeter but no beds/door. Walls require a continuous collision-bearing vertical plane along the selection's longer axis on its minimum perpendicular coordinate. Gates need doors and four solid corner columns. These are simple geometric checks, not a style/blueprint restriction. Each selection is capped at 64 blocks per axis and 32,768 blocks total; all chunks must already be loaded. No buildings overlap.
Обычным зданиям нужен сплошной пол из блоков с коллизией; границы шахты вместо этого задают зону добычи. Дома/казармы требуют настоящих пар кроватей с местом над ними, дверь, сплошную крышу по верхней плоскости и замкнутый периметр между полом и крышей (двери/окна с коллизией учитываются). Башня требует такую же крышу/периметр, но без кроватей/двери. Стена — сплошная вертикальная плоскость вдоль длинной стороны, по минимальной поперечной координате. Воротам нужны двери и четыре сплошных угловых столба. Это простая геометрия, не ограничение стиля/чертежа. Выделение ограничено 64 блоками по каждой оси и 32 768 блоками суммарно; все чанки должны быть загружены. Здания не пересекаются.

Each production slot uses a **distinct matching vanilla workstation inside the certified bounds** (mine: `smithing_table`; lumber camp: `fletching_table`). Only the first 1/2/3 workstations in coordinate order are slots. Destroying a workstation releases that slot; unloaded chunks alone do not. New miners/lumberjacks use vanilla toolsmith/fletcher appearances. Storage accepts chest, trapped chest and barrel; certified container positions are the shared deposit destinations. Barracks/fortifications certify infrastructure, not automatic soldiers.
Каждое производственное место использует **отдельный подходящий ванильный рабочий блок внутри границ** (шахта: `smithing_table`, лесопилка: `fletching_table`). Местами считаются первые 1/2/3 блока в порядке координат. Уничтожение рабочего блока освобождает место; одна выгрузка чанка — нет. Шахтёры/лесорубы выглядят как ванильные инструментальщики/лучники. Склад принимает сундуки, сундуки-ловушки и бочки; их сертифицированные координаты служат общими местами сдачи ресурсов. Казармы/укрепления сертифицируют инфраструктуру, но не создают солдат автоматически.

### Miners and lumberjacks / Шахтёры и лесорубы

Certified mines (smithing table entrance inside selected bounds) assign miners; they descend a one-block staircase then dig 1×2 branches inside the zone, refusing fluid/cave faces or unloaded blocks. Each miner keeps its excavation cursor across saves/upgrades. Certified lumber camps (fletching table) assign lumberjacks: incremental radius search finds mature log columns with nearby leaves, cuts trunks and replants the corresponding sapling (mangrove: propagule). Both work ticks 2000–8999, obey `mob_griefing`, route through wooden doors, and deliver carried resources to certified storage containers when full or at day end; full/unreachable storage retains resources, never deletes them. Clients render miners as toolsmiths, lumberjacks as fletchers (with the same vanilla trade pools).
Сертифицированная шахта (кузнечный стол у входа внутри выделенной зоны) даёт работу шахтёрам: они роют лестницу вниз и штреки 1×2 только в зоне, не вскрывая воду/лаву/пещеры или незагруженные блоки. Курсор сохраняется при перезапуске и улучшении. Лесопилка (стол лучника) даёт работу лесорубам: ограниченный пошаговый поиск зрелых стволов с листвой поблизости в радиусе поселения, рубка ствола и посадка соответствующего саженца (для мангров — ростка). Рабочие часы 2000–8999; соблюдают `mob_griefing`, используют двери и доставляют добычу на сертифицированный склад при заполнении инвентаря/в конце дня. При недоступном/полном складе предметы остаются у жителя. Ванильный вид/торги: шахтёр — инструментальщик, лесоруб — лучник.

**Worker safety:** miners pause at unloaded neighbours; permanently unsafe branches try deterministic alternate staircase lanes and keep the last actually open waypoint. Fluids, waterlogged blocks, underwater vegetation and powder snow are unsafe faces. Changing mine bounds resets the layout cursor; level-only upgrades retain it. Lumberjacks accept only natural, nonpersistent leaves and never cut a tree whose base lies inside certified building bounds; these protections are rechecked before every cut.
**Безопасность работников:** у незагруженных соседних блоков шахтёр ждёт; для постоянно опасной ветки пробует другие детерминированные линии лестницы, сохраняя последний действительно открытый проход. Вода/лава, затопленные блоки, подводная растительность и рыхлый снег считаются опасными гранями. Изменение границ шахты сбрасывает курсор схемы; повышение только уровня его сохраняет. Лесоруб принимает только естественную листву без признака постоянства и не рубит дерево с основанием внутри сертифицированного здания; защита повторно проверяется перед каждым срубленным блоком.



## Economy / Экономика
All use `/settlement <domain> <name> <action> …`; `status` is public; mutations need the owner/operator. These manage real inventories/buildings, not free resources / реальные запасы и здания, не бесплатные ресурсы.
- `production`: `status`, `start`, `pause`, `resume`, `cancel` — physical worker recipes / рецепты живого работника.
- `commerce`: `status`, `quote`, `accept`, `cancel`, `reserve`, `payout` — emerald trade and delivery escrow / торговля изумрудами и эскроу доставки.
- `payroll`: `status`, `employ`, `stop`, `dismiss`, `pay` — funded worker contracts / обеспеченные деньгами договоры работников.
- `caravan`: `status`, `open`, `pickup`, `unload`, `cancel`, `block` — physical container-to-container transport / физическая перевозка между контейнерами; `/help settlement <domain> <name>` shows action syntax / показывает синтаксис.

## Radius growth / Рост радиуса
The radius you give at `create` is a floor. A settlement is at least **4 blocks per resident** wide (capped at 256), so a village of 30 villagers reaches radius 120. The radius only ever grows (it is saved like any other settlement field and shown by `/settlement info`); bound villagers' bed, workstation and bell claims stay inside it.
Радиус при создании — минимум. Поселение не уже **4 блоков на жителя** (максимум 256): деревня из 30 жителей получает радиус 120. Радиус только растёт, сохраняется как остальные поля и виден в `/settlement info`; привязанные жители занимают кровати, рабочие блоки и колокол в его пределах.

## Breeding / Размножение
Vanilla rules: a villager is willing with **12 food points** (bread 4; carrot, potato, beetroot 1 each; wheat 0) and a **free bed with room for a baby** (a bed nobody has claimed, other than the parent's own) within reach and the settlement radius. Food comes from the village itself: working **farmers harvest mature wheat, carrots, potatoes and beetroot within 4 blocks of where they stand, replant them and bake 3 wheat into 1 bread**; a villager holding more than 24 of an item shares the surplus with neighbours who are short. A player can also drop food at villagers.
Правила как в ваниле: готовность — **12 очков еды** (хлеб 4; морковь, картофель, свёкла по 1; пшеница 0) и **свободная кровать с местом для ребёнка** (не занятая никем, кроме собственной кровати родителя) в пределах досягаемости и радиуса поселения. Еду даёт сама деревня: работающие **фермеры собирают спелую пшеницу, морковь, картофель и свёклу в 4 блоках от себя, пересаживают и пекут хлеб из 3 пшениц**; у кого больше 24 штук — делится с нуждающимися. Еду можно и бросить жителям.

Bound farmers harvest only while assigned to a certified Farmer workshop slot. A saved Farmer profession without a job keeps its trade offers but does not harvest crops; certify enough composter slots for the farmers who should feed the settlement.
Привязанный фермер собирает урожай только с назначенным местом в сертифицированной мастерской Farmer. Сохранённая профессия без рабочего места сохраняет торговые предложения, но не даёт права собирать урожай; сертифицируйте достаточно мест с компостерами для фермеров, которые кормят поселение.
