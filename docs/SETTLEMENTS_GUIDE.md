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
Adults seek a free workstation during day ticks **2000–8999**, within **48 blocks**, with a walkable path. Keep the village loaded for a few days on slow servers.
Взрослый ищет свободный рабочий блок днём, тики **2000–8999**, в пределах **48 блоков** с проходимым путём. На слабом сервере держите деревню загруженной несколько игровых дней.
Unbound villagers need no settlement command; bound villagers search only inside their settlement. Nitwits never work; traded villagers keep their profession.
Непривязанным команды не нужны; привязанные ищут только внутри поселения. Бездельники (nitwit) не работают; торговавшие сохраняют профессию.

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
