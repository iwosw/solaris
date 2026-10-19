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

## Villager jobs / Профессии жителей
Adults seek a free workstation during day ticks **2000–8999**, with a walkable path. Unbound villagers search within **48 blocks**; bound villagers can claim loaded jobs anywhere inside their settlement radius. Keep the village loaded for a few days on slow servers.
Взрослый ищет свободный рабочий блок днём, тики **2000–8999**, с проходимым путём. Непривязанные жители ищут в пределах **48 блоков**, привязанные могут занимать загруженные рабочие места во всём радиусе поселения. На слабом сервере держите деревню загруженной несколько игровых дней.
Nitwits never work; traded villagers keep their profession. A villager keeps its workstation while sleeping elsewhere or while the workstation chunk is unloaded, and returns the next workday. A destroyed or stolen workstation releases its claim; an unloaded chunk alone does not.
Бездельники (nitwit) не работают; торговавшие сохраняют профессию. Житель помнит рабочее место, когда спит в другом месте или чанк с рабочим блоком выгружен, и возвращается в следующий рабочий день. Уничтоженный или занятый другим жителем блок освобождает привязку; одна выгрузка чанка — нет.

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
