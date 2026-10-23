# Vanilla compatibility — Minecraft Java 26.1.2

**Draft prerelease; not a drop-in vanilla replacement.** This matrix describes
`solaris-v2` (`mc-edge` + `mc-core`) at **`0a86cfff`**, reviewed 2026-10-23.
It distinguishes implemented mechanics from a complete, naturally obtainable
survival experience. In particular, working portal blocks do **not** imply that
a fresh world generates strongholds, fortresses or End cities.

## Summary

Counts are feature rows below, not percentages of Minecraft, tests or mobs.
A broad `partial` row must not be counted as fully working.

| Status | Rows |
|---|---:|
| `works` | 4 |
| `partial` | 82 |
| `missing` | 13 |
| `n/a` | 2 |
| **Total** | **101** |

- **`works`**: the explicitly bounded behavior has retained execution evidence;
  this does not certify adjacent features or every input.
- **`partial`**: implementation exists, but a concrete parity gap or a stated
  evidence gap remains. Lack of a full gate is not proof the feature is absent.
- **`missing`**: the named runtime behavior is absent, not merely untested.
- **`n/a`**: a Solaris/client extension, not a vanilla-server parity requirement.
- **tested with real client — RC1** marks the retained native Java-client pass
  described below. **RC-H** marks historical real-client evidence only; it is
  explicitly not native-runtime acceptance. **RC-G** means a real-client gate
  exists but a current pass was not established. Protocol bots, unit tests,
  vanilla-server oracles and a prepared manifest are not real-client passes.

A test-file citation means coverage exists in source, **not that this review ran
it**. File/line references are for the pinned tree. The October 23 audits were
static inspections of an earlier tree (`286c275c`); their reproduction scenarios
were not executed by those auditors. [Superseded findings](#audit-provenance-and-superseded-findings)
are separated from open gaps. Unintegrated worker patches are not support claims.
See [DoD](DEFINITION_OF_DONE.md), [architecture](ARCHITECTURE.md) and
[the historical ledger's runtime warning](VALIDATION_LEDGER.md#L1-L8).

## Protocol, login, chat and commands

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| 26.1.2 protocol, status ping, Configuration and Play | `partial` | [Protocol/version contract](OPERATING.md#L42-L50); [configuration.rs:76](../crates/mc-edge/src/configuration.rs#L76); **tested with real client — [RC1](#real-client-evidence)** proves a join and narrow survival route | Not every advertised interaction is handled: [SC-03], [SC-06]. Keepalive and total pre-play deadline gaps: [Protocol SP-04], [Protocol SP-05]. |
| Offline login identity | `partial` | [login.rs:244–254](../crates/mc-edge/src/login.rs#L244-L254); [RC1](#real-client-evidence) uses an isolated non-operator client | Offline UUID derives from the chosen name, not verified ownership. Session cleanup/cap races and optional password-gate identity squatting: [AUTH-01], [AUTH-02], [AUTH-03]. |
| Online mode / Mojang authentication | `partial` | [login.rs:174–243](../crates/mc-edge/src/login.rs#L174-L243): encryption challenge, session verifier, verified UUID, failure disconnect | Fail-closed verifier exists; no retained live Mojang-account gate used here. Admission/session defects remain [AUTH-01], [AUTH-02], not a demonstrated online identity bypass. |
| Velocity modern forwarding | `partial` | [login.rs:132–173](../crates/mc-edge/src/login.rs#L132-L173): verified forwarding and name agreement | Trusted-proxy path, not protocol translation; no end-to-end proxy/client receipt established here. |
| Plain chat, private messages, `/me`, `/say`, signed commands | `partial` | [commands.rs:349–370](../crates/mc-core/src/commands.rs#L349-L370); [spam/mute policy](OPERATING.md#L431-L450) | Vanilla client's signed message-command packet is discarded ([SC-03]); legal long commands hit chat's shorter limit ([SC-05]). SC-01/04 have focused fixes, not blanket chat certification. |
| Operator/gameplay commands, selectors, `/execute`, teams, scoreboard, titles, bossbars | `partial` | [native parser](../crates/mc-core/src/commands.rs#L303-L385); [operating contract](OPERATING.md#L462-L499); [command execution tests](../crates/mc-core/src/command_execution_tests.rs) | `/execute in` only wraps teleport; `/locate` only finds generated villages; bossbar Anvil export is unwired. [SC-05]. Advertised roots are not a full vanilla syntax matrix. |
| Map-making commands: `/clone`, `/data`, `/function`, `/datapack`, `/schedule`, `/place`, `/fillbiome`, `/damage`, `/attribute`, `/loot`, `/item`, `/recipe`, `/trigger`, `/tag`, `/spreadplayers`, `/rotate` | `missing` | [native parser's supported dispatch](../crates/mc-core/src/commands.rs#L303-L385); [command tree](../crates/mc-edge/src/command_tree.rs) | No corresponding native command implementations; no audit ID assigned to this inventory of missing commands. |
| Command blocks: impulse, chain, repeating | `missing` | [supported command surface](../crates/mc-core/src/commands.rs#L303-L385); [README exclusions](../README.md#L56-L57) | No block-driven command execution; an imported command-block block state is not an executable command block. |
| Books/quills: editing and signing | `missing` | [SC-06] traces the absent EditBook packet/input route | Client-entered pages/title are not saved or converted to an authoritative signed book ([SC-06]). |

## Movement, physics and combat

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| Walking, sprinting, jumping, collision, stairs and fall damage | `partial` | [movement.rs:179–214](../crates/mc-core/src/tick/input/movement.rs#L179-L214); [mining support regression](../crates/mc-core/src/tick/mining_tests.rs#L433); **tested with real client — [RC1](#real-client-evidence)**, supported movement/mining only | Hay/bed/slime landing response and flowing/waterlogged fall cancellation differ ([VS-08], [VS-09]); connected-block geometry also differs ([VB-26]). |
| Diagonal sprint speed acceptance (bounded oracle scenario) | `works` | [26.1.2 player oracle result](QUIRK_PARITY.md#L113-L115): zero corrections on both servers | Only the named diagonal-sprint scenario, not complete movement or graphical-client proof. |
| Water swimming, buoyancy, drowning, bubble-column transport | `partial` | [movement owner](../crates/mc-core/src/tick/input/movement.rs); [survival owner](../crates/mc-core/src/survival.rs); [pearl/stasis limitation](OPERATING.md#L516-L525); [RC-G](#real-client-evidence) | Swimming exhaustion and pose/fluid drowning sample differ ([VS-10], [VS-16]); bubble-column entity physics is absent. Broad swim-feel gate remains blocked. |
| Elytra/gliding and firework boost | `partial` | [movement limit/flight handling](../crates/mc-core/src/tick/input/movement.rs#L168-L193); [elytra_tests.rs](../crates/mc-core/src/elytra_tests.rs) | Native coverage is not a full flight/landing client gate; naturally generated elytra ships are absent ([W04]). |
| Boats, minecarts, mounts, passenger travel | `partial` | [vehicles_tests.rs](../crates/mc-core/src/vehicles_tests.rs); [mounts_tests.rs](../crates/mc-core/src/mounts_tests.rs) | Rail connection/power shape ([VB-08], [VB-09]), mounted portal refusal and vehicle cooldown ([W09], [W12]); horse/donkey breeding ([VC-13]). |
| Melee: cooldown, critical hits, sweep, armor, enchantments, knockback, mace | `partial` | [combat_tests.rs](../crates/mc-core/src/combat_tests.rs); [mace_tests.rs](../crates/mc-core/src/mace_tests.rs); [combat audit's bounded positive checks][combat-audit] | Wrong critical eligibility ([VC-03]); mobs hit through walls ([VC-04]); Creative target immunity is missing ([VS-01]). No broad native real-client combat pass established. |
| Shields, bows, crossbows, tridents and projectile deflection | `partial` | [shield admission:834–866](../crates/mc-core/src/tick/damage.rs#L834-L866); [weapon tests](../crates/mc-core/src/projectiles/weapons_tests.rs) | Explosion blocking, Piercing bypass, crossbow critical bonus and arrow rebound differ ([VC-01], [VC-02], [VC-08], [VC-18]). |
| Explosions, fire, lava, suffocation, falling-object hazards | `partial` | [contact_hazards_tests.rs](../crates/mc-core/src/contact_hazards_tests.rs); [falling_tests.rs](../crates/mc-core/src/falling_tests.rs) | Anvil/stalactite impact damage absent ([VB-17]); rain extinguishing and lava afterburn differ ([VS-11], [VS-19]); fire spread/waterlogging differ ([VB-28], [VB-29]). |

## Mobs by family and villagers

A registry entry, spawn egg or `species_tests` lifecycle pass is not proof of a
mob's complete AI. The [species baseline list](../crates/mc-core/src/species_tests.rs#L10-L83)
and [spawn/move/death test](../crates/mc-core/src/species_tests.rs#L145-L218)
cover broad representation; family-specific gaps remain below. No unlisted
species is implicitly certified.

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| Natural spawning, mob caps, despawn and loot/XP | `partial` | [natural_spawn_parity_tests.rs](../crates/mc-core/src/natural_spawn_parity_tests.rs); [natural_spawn_breadth_tests.rs](../crates/mc-core/src/natural_spawn_breadth_tests.rs); [mob fall/loot oracle](QUIRK_PARITY.md#L107-L115) | Spawn-proof glass/slabs, baby-zombie loot and equipment XP differ ([VC-05], [VC-09], [VC-20]); farm kill-chamber drift remains [documented](QUIRK_PARITY.md#L124-L126). |
| Farm animals: cows/mooshrooms, pigs, sheep, chickens, rabbits; breeding | `partial` | [entity_life_tests.rs](../crates/mc-core/src/entity_life_tests.rs); [food temptation and juvenile following](../crates/mc-core/src/tick/passive_follow.rs) | VC-06 has focused native regressions for both-hand food temptation and baby following. Direct steering still does not provide complete obstacle navigation; lifecycle coverage is not full species/client behavior. |
| Chicken egg production rate (bounded oracle farm) | `works` | [egg RNG fix and passing farm receipt summary](QUIRK_PARITY.md#L106-L115) | Only `farm-chicken-eggs`, not all chicken AI, breeding or farm layouts. |
| Tameable/neutral animals: wolves, cats, parrots, foxes, ocelots | `partial` | [taming tests:4,44,97,149](../crates/mc-core/src/taming_tests.rs#L4); [blueprint-driven food following](../crates/mc-core/src/tick/passive_follow.rs) | Ownership/sitting/trust and food-following coverage is narrower than owner follow/defense, teleport, shoulder and complete species behavior; no broad native client gate. |
| Mounts: horses/donkeys/mules, llamas, camels, striders, happy ghasts | `partial` | [mounts_tests.rs](../crates/mc-core/src/mounts_tests.rs); [horse/donkey courtship and mule regression](../crates/mc-core/src/entity_life_tests.rs); [pair admission](../crates/mc-core/src/entity_life_breeding.rs) | VC-13 has a focused native fix for both parent orders with sterile offspring. Mounted portal transfer remains [W09]; happy-ghast riding is absent from mount admission, and generic lifecycle is not special mount parity. |
| Other terrestrial passives: pandas, polar bears, goats, armadillos, sniffers | `partial` | [species list:19–23](../crates/mc-core/src/species_tests.rs#L19-L23), [lifecycle:146](../crates/mc-core/src/species_tests.rs#L146) | Baseline movement/breeding/death coverage only; no complete per-species special-behavior or real-client gate established. No dedicated audit ID. |
| Aquatic/amphibious: fish, squid/glow squid, dolphins, nautiluses, axolotls, frogs, turtles, tadpoles | `partial` | [species list:38–49](../crates/mc-core/src/species_tests.rs#L38-L49); [dolphin grace:69](../crates/mc-core/src/dolphin_tests.rs#L69), [item play:171](../crates/mc-core/src/dolphin_tests.rs#L171) | Dolphin-specific tests do not establish parity for every aquatic species; broad navigation, breeding and client presentation remain unproven. |
| Bees and beehives | `partial` | [nectar/sting:283](../crates/mc-core/src/bees_tests.rs#L283); [hive WAL custody:332](../crates/mc-core/src/bees_tests.rs#L332), [silk-touch custody:574](../crates/mc-core/src/bees_tests.rs#L574) | Native action/persistence tests, not complete pollination/pathing/farm or real-client acceptance. No dedicated open audit ID. |
| Allays | `partial` | [collection/delivery:86](../crates/mc-core/src/allay_tests.rs#L86); [partial-pickup recovery:6](../crates/mc-core/src/allay_tests.rs#L6) | INT-02 has a focused recovery fix; broad delivery/navigation/client parity still unproven. See [superseded findings](#audit-provenance-and-superseded-findings). |
| Undead walkers: zombies, husks, drowned and newer variants | `partial` | [hostile_tests.rs](../crates/mc-core/src/hostile_tests.rs); [species list:50–57](../crates/mc-core/src/species_tests.rs#L50-L57) | Wall melee, blocked direct pursuit, baby loot and equipment XP ([VC-04], [VC-15], [VC-09], [VC-20]). |
| Skeleton archers: skeletons, strays, bogged, parched; wither skeletons | `partial` | [hostile.rs:314–374](../crates/mc-core/src/hostile.rs#L314-L374); [hostile_tests.rs](../crates/mc-core/src/hostile_tests.rs); [species list:58–62](../crates/mc-core/src/species_tests.rs#L58-L62) | Broad pathing/loot parity not established; equipped-mob XP ([VC-20]) and airborne farm drift ([quirk findings](QUIRK_PARITY.md#L124-L126)). |
| Creepers | `partial` | [hostile.rs:423–480](../crates/mc-core/src/hostile.rs#L423-L480); [hostile_tests.rs](../crates/mc-core/src/hostile_tests.rs) | Flint-and-steel/fire-charge ignition absent ([VC-14]); explosion shield interaction differs ([VC-01]). |
| Spiders/cave spiders, silverfish, endermites | `partial` | [species list:51–64](../crates/mc-core/src/species_tests.rs#L51-L64); [hostile_tests.rs](../crates/mc-core/src/hostile_tests.rs) | Spider hostility uses global night rather than local light/retaliation ([VC-07]); other arthropod special behavior not fully gated. |
| Slimes and magma cubes | `partial` | [entity_loot.rs split path](../crates/mc-core/src/entity_loot.rs); [species lifecycle](../crates/mc-core/src/species_tests.rs#L146) | Walker-style motion instead of hopping ([VC-19]); split/death coverage does not fix locomotion. |
| Endermen | `partial` | [enderman_tests.rs](../crates/mc-core/src/enderman_tests.rs); [combat audit positive checks][combat-audit] | Dry supported teleport/projectile immunity/pumpkin/water checks exist; full stare, carrying, navigation and client cadence unproven. |
| Witches | `partial` | [special_mobs/shooters.rs:205–249](../crates/mc-core/src/special_mobs/shooters.rs#L205-L249); [special_mobs_tests.rs](../crates/mc-core/src/special_mobs_tests.rs) | Missing magic resistance and Weakness throw ([VC-10], [VC-17]). |
| Piglins/brutes, zombified piglins, hoglins/zoglins; bartering | `partial` | [accepted barter:75](../crates/mc-core/src/piglin_tests.rs#L75); [species baseline](../crates/mc-core/src/species_tests.rs#L50-L71) | Native barter/goal coverage only; complete anger, hunting and transformation parity unproven. Missing Nether habitats/resources are separate [W02], [W03], [W05]. |
| Blaze and ghast ranged mobs | `partial` | [blaze burst:130](../crates/mc-core/src/special_mobs_tests.rs#L130); [ghast charging:159](../crates/mc-core/src/special_mobs_tests.rs#L159) | Focused combat tests do not supply fortress/spawner progression ([W02]) or full flight/client parity. |
| Guardians/elder guardians, shulkers, breeze | `partial` | [beam:348](../crates/mc-core/src/special_mobs_tests.rs#L348), [shulker shot:516](../crates/mc-core/src/special_mobs_tests.rs#L516), [breeze:284](../crates/mc-core/src/special_mobs_tests.rs#L284) | Special attack tests are not full encounter acceptance; monuments/trial chambers/End cities are not naturally generated ([W01], [W04], [structure contract](OPERATING.md#L489-L499)). |
| Illagers/raiders: pillagers, vindicators, evokers, vexes, ravagers; raids | `partial` | [raid wave/victory/discount/gift:178](../crates/mc-core/src/raids_tests.rs#L178), [difficulty waves:251](../crates/mc-core/src/raids_tests.rs#L251) | Native raid tests, not a complete vanilla-client raid or navigation/farm oracle; non-village structure encounters are unavailable in fresh generation. |
| Warden/sculk and creaking | `partial` | [warden anger:82](../crates/mc-core/src/warden_tests.rs#L82), [sonic:393](../crates/mc-core/src/special_mobs_tests.rs#L393); [creaking gaze:33](../crates/mc-core/src/creaking_tests.rs#L33) | No full naturally generated encounter gate; underground sculk features absent ([W08]), pale-oak growth absent ([VB-23]). |
| Iron/snow/copper golems | `partial` | [golems_tests.rs](../crates/mc-core/src/golems_tests.rs); [copper_tests.rs](../crates/mc-core/src/copper_tests.rs); [iron loot oracle](QUIRK_PARITY.md#L109-L115) | Iron-ingot healing, gossip summoning and attack damage/difficulty differ ([VC-11], [VC-12], [VC-16]). |
| Wither boss | `partial` | [ritual, restart, heads, armor, grief, star:626](../crates/mc-core/src/special_mobs_tests.rs#L626); [LOS/immunity:988](../crates/mc-core/src/special_mobs_tests.rs#L988) | Native focused tests, not complete real-client boss acceptance; naturally generated fortress resources absent ([W02]). |
| Bats/phantoms and command-only giant/illusioner | `partial` | [species list](../crates/mc-core/src/species_tests.rs#L10-L83); [giant:64](../crates/mc-core/src/rare_mobs_tests.rs#L64), [illusioner:136](../crates/mc-core/src/rare_mobs_tests.rs#L136) | Generic lifecycle and named special tests do not certify bat/phantom AI. Giant/illusioner command-only availability is not itself a vanilla gap. |
| Villagers: jobs, beds, POIs, schedules, breeding, gossip, golems | `partial` | [villager/brain.rs](../crates/mc-core/src/villager/brain.rs); [villager persistence tests](../crates/mc-core/src/villager/persistence_tests.rs); [combat audit][combat-audit] | Missing gossip-triggered golems ([VC-12]); farmer harvest about 2.4× vanilla in [breeder oracle](QUIRK_PARITY.md#L127). Settlement residency is an extension, not vanilla proof. |
| Villager/wandering-trader trade UI, offers, restock and prices | `partial` | [villager/trades_tests.rs](../crates/mc-core/src/villager/trades_tests.rs); [merchant_tests.rs](../crates/mc-core/src/tick/merchant_tests.rs); [trader persistence](../crates/mc-core/src/level_metadata_trader_tests.rs) | Native trading/restock coverage; complete vanilla offer/demand/discount/UI and survival acquisition gate not established. No dedicated audit ID. |

## Items, stations and containers

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| Mining, tool speed/durability, item drops and placement | `partial` | [mining tests](../crates/mc-core/src/tick/mining_tests.rs); **tested with real client — [RC1](#real-client-evidence)**: grass break, dirt pickup/place and debit | VI-01 fixed with timed admission; zero-hardness wear ([VI-12]), sneak placement against containers ([VI-11]) and shape/support gaps remain. |
| Item stacking, inventory/cursor custody, pickup and drop | `partial` | [inventory/click_tests.rs](../crates/mc-core/src/inventory/click_tests.rs); [player_tests.rs](../crates/mc-core/src/player_tests.rs); **tested with real client — [RC1](#real-client-evidence)**, one dirt stack | Focused INT-01..05 fixes do not certify every malformed click, component or restart interaction; see [closure notes](#audit-provenance-and-superseded-findings). |
| Shaped/shapeless crafting, repair and recipe book | `partial` | [inventory/crafting.rs:10–92](../crates/mc-core/src/inventory/crafting.rs#L10-L92); [recipe_book_tests.rs](../crates/mc-core/src/inventory/recipe_book_tests.rs); [RC-G](#real-client-evidence), oak-log recipe only | Special recipes omitted ([VI-04]); repair strips curses and rejects fresh+damaged inputs ([VI-06], [VI-14]); broad recipe/client gate not established. |
| Furnaces, blast furnaces, smokers and campfires | `partial` | [inventory/furnace_tests.rs](../crates/mc-core/src/inventory/furnace_tests.rs); [campfire_tests.rs](../crates/mc-core/src/campfire_tests.rs); [item audit positive checks][items-audit] | Recipe-book placement absent ([VI-08]); banked XP lost on restart and block break ([VI-09], [VI-10]); broad station-client gate pending. |
| Brewing and potion batches | `partial` | [400-tick batch:136](../crates/mc-core/src/brewing_tests.rs#L136), [cancellation:187](../crates/mc-core/src/brewing_tests.rs#L187), [checkpoint:285](../crates/mc-core/src/brewing_tests.rs#L285) | Native recipes/timing/custody tests, not every potion/UI path; generated fortress wart/blaze supply absent ([W02]). |
| Enchanting table and enchantment effects | `partial` | [inventory/enchanting_tests.rs](../crates/mc-core/src/inventory/enchanting_tests.rs); [enchantment_tick_tests.rs](../crates/mc-core/src/enchantment_tick_tests.rs) | Unbreaking consumes the enchanting offer seed, enabling free rerolls ([VI-07]); damage exceptions differ ([VS-07], [VC-02]). |
| Anvil, smithing, stonecutter and grindstone | `partial` | [inventory/anvil.rs:42–171](../crates/mc-core/src/inventory/anvil.rs#L42-L171); [craft_stations_tests.rs](../crates/mc-core/src/inventory/craft_stations_tests.rs); [player station tests](../crates/mc-core/src/player/stations_tests.rs) | Bounded native repair/merge/cost/station transactions; no full actual-client station matrix. Falling anvil damage separately absent ([VB-17]). |
| Food, drink, effects and item remainders | `partial` | [consumables_tests.rs](../crates/mc-core/src/consumables_tests.rs); [effects_tests.rs](../crates/mc-core/src/effects_tests.rs); [item audit][items-audit] | Harmful foods omit negative effects ([VI-05]); food nutrition alone is not consumable parity. |
| Chest/double chest, barrel, shulker box, shared menus and drag/shift clicks | `partial` | [chests_tests.rs](../crates/mc-core/src/chests_tests.rs); [shulker_tests.rs](../crates/mc-core/src/shulker_tests.rs); **tested with real client — [RC-H](#real-client-evidence)**, historical shared-chest routes only | Blocked lids still open ([VI-13]); sneak-use differs ([VI-11]). INT-01/04 fixed; historical client evidence does not revalidate native containers. |
| Vanilla structure chest loot catalog | `partial` | [startup compiler](../crates/mc-data/src/loot/chest_26_1_2.rs), [functions](../crates/mc-data/src/loot/chest_functions.rs), [regressions](../crates/mc-data/src/loot/chest_26_1_2_tests.rs) | Loads all 56 vanilla chest tables, 6 archaeology tables and 15 hero gifts from the selected content root. Supports nested trial rewards, ordered functions/conditions, random and level enchantments, potions, stew, horns and preserved text/components. Unknown functions/conditions fail at load with table/path diagnostics. Exploration-map tables require the native saved-map callback: missing context fails explicitly without consuming RNG; a completed lookup with no destination omits the map. Callers retain unresolved table/seed until first-open lookup completes. Fishing retains its separate context-aware evaluator; entity/tool/explosion conditions are not silently accepted as chest contexts. This is compiler/native-stack coverage, not real-client or structure-placement proof. |
| Ender-chest personal inventory | `missing` | [VI-02] traces supported-menu dispatch with no ender-chest case | Right-click does not open a persistent private inventory ([VI-02]); block placement does not implement storage. |
| Item frames/glow item frames | `missing` | [VI-03] traces item-use placement dispatch | No normal frame placement/custody loop ([VI-03]). |
| Armor stands | `partial` | [armor_stand_tests.rs](../crates/mc-core/src/armor_stand_tests.rs); [armor_stand.rs:60–183](../crates/mc-core/src/armor_stand.rs#L60-L183) | Placement/equipment/double-hit behavior has native coverage; complete damage/NBT/client combinations unproven. |
| Signs/hanging signs, beds and campfire block entities | `partial` | [signs_tests.rs](../crates/mc-core/src/signs_tests.rs); [sleep_tests.rs](../crates/mc-core/src/sleep_tests.rs); [RC-G](#real-client-evidence), regular-sign probe only | Broad hanging/waxed/styled text, restart and client visuals unproven; bed fractional-shape exits remain approximate (see survival rows). |
| Fishing, buckets/bottles, beacons, conduits, jukeboxes, trial spawners and vaults | `partial` | [fishing tests](../crates/mc-core/src/projectiles/fishing_tests.rs), [beacon](../crates/mc-core/src/beacon_tests.rs), [conduit](../crates/mc-core/src/conduit_tests.rs), [trial spawner](../crates/mc-core/src/trial_spawner_tests.rs), [vault](../crates/mc-core/src/vault_tests.rs) | Focused native mechanics, not all item use or full survival encounters. No generated trial chambers/monuments; waterlogged interaction gaps [VB-13], [VB-14]. |

## Blocks, redstone, fluids, light and farming

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| Placement/support, doors/trapdoors, fences/walls/panes and stairs | `partial` | [placement_tests.rs](../crates/mc-core/src/placement_tests.rs); [placement_shapes.rs:79–117](../crates/mc-core/src/placement_shapes.rs#L79-L117) | Narrow torch support, connected states and door hinges differ ([VB-25], [VB-26], [VB-30]); diodes survive support loss ([VB-24]). |
| Redstone dust, solid-block conduction, levers/buttons/plates, torches and lamps | `partial` | [signal.rs](../crates/mc-core/src/redstone/signal.rs), [gap regressions](../crates/mc-core/src/redstone/gap_tests.rs), [redstone_tests.rs](../crates/mc-core/src/redstone/redstone_tests.rs) | Native strong/weak one-block conduction, directional dust and indirect consumer wakes address [VB-02], [VB-03]. Torch burnout and exact update-order parity remain outside this implementation. |
| Repeaters and comparators | `partial` | [gap regressions](../crates/mc-core/src/redstone/gap_tests.rs), [analog inputs](../crates/mc-core/src/redstone/signal.rs), [redstone_tests.rs](../crates/mc-core/src/redstone/redstone_tests.rs) | Admitted short pulses are extended; cauldrons/composters/cake/hives and through-solid container reads address [VB-04], [VB-05], [VB-06]. Additional analog sources (lecterns/chiseled bookshelves/item frames) and support-loss integration [VB-24] remain separate work. |
| Observers, daylight detectors and sculk sensors | `partial` | [observer pulse audit][blocks-audit]; [daylight regressions](../crates/mc-core/src/redstone/gap_tests.rs); [vibration_tests.rs](../crates/mc-core/src/vibration_tests.rs); [sculk_tests.rs](../crates/mc-core/src/sculk_tests.rs) | Daylight uses local sky, weather darkening and dimension-specific behavior ([VB-07]); observer/sculk fixtures are not whole redstone update-order parity. |
| Pistons/sticky pistons, slime/honey transport, quasi-connectivity | `partial` | [piston_tests.rs](../crates/mc-core/src/redstone/piston_tests.rs); [QC oracle pass](QUIRK_PARITY.md#L113-L115) | Instant movement has no short-pulse spit or swept entity push ([VB-10], [VB-11]); moving-piston dupe window absent. Protected-zone VB-01 has a focused fix. |
| TNT, carpet/rail duping, BUD/update-order and zero-tick contraptions | `partial` | [quirk scenario families](QUIRK_PARITY.md#L81-L94); [known divergences](QUIRK_PARITY.md#L117-L128); [tnt_tests.rs](../crates/mc-core/src/tnt_tests.rs) | Oracle-found TNT BUD duper yields one primed TNT instead of two; carpet/rail dupers share missing moving-piston model. Fluid destroy-list order differs. Scenario existence is not a pass. |
| Hoppers, droppers, dispensers and crafter | `partial` | [containers_tests.rs](../crates/mc-core/src/redstone/containers_tests.rs); [crafter_tests.rs](../crates/mc-core/src/crafter_tests.rs); [QC note](QUIRK_PARITY.md#L128) | No dispenser/dropper QC oracle scenario; indirect powering broken ([VB-02]); blanket transfer/item-action/client parity not established. |
| Rails: ordinary, powered, detector and activator | `partial` | [native rail topology](../crates/mc-core/src/redstone/rails.rs), [gap regressions](../crates/mc-core/src/redstone/gap_tests.rs), [vehicles_tests.rs](../crates/mc-core/src/vehicles_tests.rs) | Neighbor-driven straight/curve/slope selection and reciprocal endpoint power propagation address [VB-08], [VB-09]. Isolated initial placement orientation is a placement integration dependency; exact junction update order and moving-piston rail duping remain outside these fixtures. |
| Water/lava ticks, source creation, buckets and waterlogging | `partial` | [fluid_parity_tests.rs](../crates/mc-core/src/fluid_parity_tests.rs); [fluid_ticks.rs](../crates/mc-core/src/fluid_ticks.rs); [RC-G](#real-client-evidence), source-bucket probe only | Missing shortest-descent selection and face occlusion; lava erases waterlogged blocks ([VB-12], [VB-13], [VB-14]). |
| Skylight/block light, edits and chunk light delivery | `partial` | [light_tests.rs](../crates/mc-core/src/light_tests.rs); [light.rs:360–392](../crates/mc-core/src/light.rs#L360-L392); [block audit][blocks-audit] | Directional slab/stair occlusion and shape-only invalidation missing ([VB-27]); a lit chunk packet is not visual parity. |
| Sand/gravel, concrete powder, falling anvils/stalactites | `partial` | [falling_tests.rs](../crates/mc-core/src/falling_tests.rs); [falling.rs](../crates/mc-core/src/falling.rs) | Non-solid support, delayed underwater hardening and no impact damage ([VB-15], [VB-16], [VB-17]). |
| Wheat and sugar-cane production rates (bounded oracle farms) | `works` | [26.1.2 crop/cane oracle results](QUIRK_PARITY.md#L104-L115) | Only named rate fixtures; not all crops, soil, bone meal or farm layouts. |
| Farming, farmland, grass, stems, bamboo, trees and bone meal | `partial` | [farming_tests.rs](../crates/mc-core/src/farming_tests.rs); [farming_environment_tests.rs](../crates/mc-core/src/farming_environment_tests.rs); [plants_tests.rs](../crates/mc-core/src/plants_tests.rs); [composter_tests.rs](../crates/mc-core/src/composter_tests.rs) | Waterlogged/rain irrigation, covered-soil decay, grass/mycelium light/survival, supported fruit, bamboo bone meal and cherry/2×2 pale oak are implemented. Player landing tramples farmland; mob landing remains. Kelp and growing vines have bounded tip growth; ordinary vines currently extend downward only. Mushroom spread/huge growth are bounded simplified shapes. Azalea/mangrove trees, full decorative/fungus/moss bone-meal coverage and exact tree shapes remain incomplete; blocked bone-meal payment differs from vanilla. |

## World generation, dimensions and progression

These rows describe **newly generated** terrain. Imported structure blocks do
not count as native generation. Vanilla-like density/RNG differences alone are
not gaps; missing resources, biomes and structures are.

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| Overworld terrain, climate/3D biomes, caves, ores and decoration | `partial` | [terrain.rs](../crates/mc-worldgen/src/terrain.rs); [world audit positive checks][world-audit]; [terrain_tests.rs](../crates/mc-core/src/terrain_tests.rs) | No cave aquifers/deep lava; badlands lack terracotta strata; cave biome IDs lack defining underground features ([W06], [W07], [W08]). |
| Overworld villages and structure lookup | `partial` | [production `/locate` contract:489–499](OPERATING.md#L489-L499); [structures.rs](../crates/mc-worldgen/src/structures.rs); [village generation](VILLAGE_GENERATION.md) | Five village variants only; no general structure placement parity ([W01]). Registry suggestions are not generated structures. |
| Overworld non-village structures: strongholds, mineshafts, temples, pyramids, igloos, monuments/ruins, outposts, ruined portals, shipwrecks, huts, mansions, treasure, ancient cities, trail ruins, trial chambers | `missing` | [only-villages production contract](OPERATING.md#L492-L499); [W01] | No generated stronghold End portal room or other listed structure content. Command-placed trial blocks are not generated trial chambers. |
| Nether base terrain | `partial` | [NetherGenerator::generate:137–178](../crates/mc-worldgen/src/nether.rs#L137-L178) | Floor/roof/lava height field exists, but no volumetric cavern/biome landscape ([W05]). |
| Nether biome-specific landscape, ores and structures (fortresses, bastions, fossils, ruined portals) | `missing` | [nether.rs:133–178](../crates/mc-worldgen/src/nether.rs#L133-L178); [W02], [W03], [W05] | Only Nether wastes with base materials; no fortress/wart progression, quartz/gold/ancient debris or non-wastes decoration. |
| End central island and obsidian pillars | `partial` | [end.rs:187–230](../crates/mc-worldgen/src/end.rs#L187-L230); [first-entry pillars/crystals test](../crates/mc-core/src/end_fight/tests.rs#L86) | Central-island generation only; no complete End landscape ([W04]). |
| End outer islands, chorus, cities/ships and naturally obtainable elytra | `missing` | [W04]; [end.rs:121–135](../crates/mc-worldgen/src/end.rs#L121-L135) | Gateway emergency landing island is not outer terrain/city generation ([W04]). |
| Nether/End portals, dimension transfer and portal chunk loading | `partial` | [portals_tests.rs](../crates/mc-core/src/portals_tests.rs); [dimensions_tests.rs](../crates/mc-core/src/dimensions_tests.rs); [world audit portal checks][world-audit] | Passenger assemblies refused, 300-tick destination lease absent, empty-vehicle cooldown wrong ([W09], [W10], [W12]). End frame logic does not restore missing strongholds ([W01]). |
| Ender dragon fight, death XP, exit fountain, egg, gateways and crystal respawn | `partial` | [fight tests:86,165,248,292,469](../crates/mc-core/src/end_fight/tests.rs#L86); [end_fight.rs](../crates/mc-core/src/end_fight.rs) | Native state-machine/restart tests, no complete actual-client dragon fight established. Fresh-world access and post-fight exploration blocked by [W01], [W04]. |
| Natural Overworld→Nether→End survival progression | `partial` | [portal/fight implementations][world-audit]; [W01], [W02], [W03], [W04] | Cannot complete the ordinary generated-world route: no stronghold entrance/fortress progression; no ancient debris or End-city rewards. Imported/admin-supplied content is not a fix for fresh-world generation. |

## Survival loop and game rules

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| Hunger, saturation, exhaustion, regeneration and starvation | `partial` | [survival_tests.rs](../crates/mc-core/src/survival_tests.rs); [survival.rs:289–379](../crates/mc-core/src/survival.rs#L289-L379) | Peaceful recovery/hunger, Protection against starvation and swimming cost differ ([VS-03], [VS-07], [VS-16]); harmful foods [VI-05]. |
| XP levels, orbs, Mending and `/experience` | `partial` | [experience_orbs.rs](../crates/mc-core/src/experience_orbs.rs); [survival_tests.rs](../crates/mc-core/src/survival_tests.rs); [SC-02 fix receipt][commands-fix] | No death orbs ([VS-04]); equipped-mob bonus absent ([VC-20]); furnace bank lifetime gaps ([VI-09], [VI-10]). Max-level overflow is fixed, not a current gap. |
| Death, drops, keep-inventory, death messages and respawn | `partial` | [player.rs death/respawn owner](../crates/mc-core/src/player.rs); [dimensions_tests.rs](../crates/mc-core/src/dimensions_tests.rs); [RC-G](#real-client-evidence), broad combat gate blocked | XP deletion, retained fire, invalid spawn retention/radius and victim-only generic death message ([VS-04], [VS-06], [VS-14], [VS-18], [VS-20]). |
| Beds/anchors: personal spawn, dimension rules and safe exits | `partial` | [dimensions_tests.rs](../crates/mc-core/src/dimensions_tests.rs); [sleep_tests.rs](../crates/mc-core/src/sleep_tests.rs); [bed exit owner](../crates/mc-core/src/sleep.rs) | Daytime spawn and both-half perimeter/lower-tier exits implemented ([VS-05], [VS-13]); fractional collision-shape exits remain approximate. Invalid bed spawn retention is tracked separately ([VS-18]). |
| Sleeping, multiplayer quorum and skipping night | `works` | [sleep.rs](../crates/mc-core/src/sleep.rs); [sleep_tests.rs](../crates/mc-core/src/sleep_tests.rs) | Native sleeping dimensions, safe wake teleport, quorum and already-clear weather preservation cover [VS-12], [VS-17], [VS-21]. |
| Time, rain/snow and commanded thunder | `partial` | [weather_tests.rs](../crates/mc-core/src/weather_tests.rs); [weather.rs](../crates/mc-core/src/weather.rs); [precipitation owner](../crates/mc-core/src/weather_surface.rs) | Independent retained rain/thunder/clear clocks, exposed rain extinguishing, snow accumulation, source-water freezing and precipitation cauldrons implemented. Temperature uses biome-family/altitude approximation; climate-noise and snow/ice melting parity remain unproven. |
| Natural thunderstorms and lightning | `partial` | [weather_tests.rs](../crates/mc-core/src/weather_tests.rs); [lightning owner](../crates/mc-core/src/weather_lightning.rs); [projectile outcomes tests](../crates/mc-core/src/projectiles/payloads_tests.rs) | Native scheduled storm strikes, 128-block rod attraction, rod pulses, fires and entity transformations/damage implemented ([VS-15]). Skeleton-horse traps and copper deoxidation remain absent; no real-client storm gate claimed. |
| Survival/Adventure/Creative/Spectator game modes | `partial` | [movement.rs:179–214](../crates/mc-core/src/tick/input/movement.rs#L179-L214); [damage admission:653–710](../crates/mc-core/src/tick/damage.rs#L653-L710); [survival audit][survival-audit] | Creative takes ordinary melee damage ([VS-01]); Spectator collides ([VS-02]); setting the enum is not complete mode semantics. Broad Adventure coverage unproven. |
| Peaceful/Easy/Normal/Hard difficulty and gamerules | `partial` | [survival.rs:47–63](../crates/mc-core/src/survival.rs#L47-L63); [command_rules.rs](../crates/mc-core/src/command_rules.rs); [raid difficulty test](../crates/mc-core/src/raids_tests.rs#L251) | Peaceful hunger and golem damage differ ([VS-03], [VC-16]); accepted respawn-radius rule is ineffective ([VS-14]). |
| Statistics, advancements and recipe discovery | `partial` | [statistics_tests.rs](../crates/mc-core/src/tick/statistics_tests.rs); [advancements_tests.rs](../crates/mc-core/src/advancements_tests.rs); [recipe_book_tests.rs](../crates/mc-core/src/inventory/recipe_book_tests.rs); [cultivation acceptance regression](../crates/mc-core/src/farming_environment_tests.rs) | Accepted cultivation now emits block-use advancement evidence with the clicked block and held item; rejected footprints do not award it. Trigger/source tests still do not cover every vanilla advancement. Missing special recipes, furnace-book placement and natural progression depend on their respective slices ([VI-04], [VI-08], [W01], [W02], [W04]). |

## Saves, packs, administration and plugins

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| Native world save, tick WAL, checkpoints, clean restart and crash recovery | `partial` | [native_checkpoint_crash_tests.rs](../crates/mc-core/src/native_checkpoint_crash_tests.rs); [persistence_crash_tests.rs](../crates/mc-core/src/persistence_crash_tests.rs); [durability contract](OPERATING.md#L565-L595) | Focused crash/custody tests, not universal losslessness. Furnace XP omitted ([VI-09]); unflushed acknowledged tail may roll back consistently. INT-01..05 fixes do not prove every save path. |
| Offline Anvil import/export | `partial` | [conversion contract](OPERATING.md#L565-L595); [roundtrip_tests.rs](../crates/mc-core/src/world_conversion/roundtrip_tests.rs); [world audit conversion checks][world-audit] | Nonempty protochunks served incomplete ([W11]); empty protochunk regeneration can lose opaque data ([W13]); superflat/custom dimensions refused; POI retention is opaque; bossbar export unwired. Inspect each conversion receipt. |
| Live Anvil serving / dual-write | `n/a` | [native-only architecture](ARCHITECTURE.md#L73-L95) | Deliberate storage architecture, not a missing vanilla gameplay mechanic. Use explicit offline interchange; never assume an `.mca` is the live save. |
| Vanilla datapacks: functions, scheduling, custom recipes/loot/worldgen packs | `missing` | [native command dispatch](../crates/mc-core/src/commands.rs#L303-L385); [licensed built-in content import](OPERATING.md#L19-L40) | Reading extracted built-in vanilla data is not loading arbitrary datapacks, `/reload` or a function runtime. Plugin worldgen declarations are a separate API. |
| Server-offered vanilla resource packs and required-pack policy | `partial` | [Configuration pack negotiation](../crates/mc-edge/src/resource_pack.rs); [pack wire/status regressions](../crates/mc-edge/src/resource_pack_tests.rs); [operator configuration](OPERATING.md#server-offered-vanilla-resource-packs) | HTTP(S) URL, SHA1, UUID, JSON prompt and required policy; terminal success required before Play, optional decline allowed. Real-client download/render gate remains unrun; Loader bundles are independent. |
| `server.properties` file compatibility | `partial` | [startup importer](../crates/mc-edge/src/server_properties.rs); [import regressions](../crates/mc-edge/src/server_properties_tests.rs); [mapping and precedence](OPERATING.md#importing-serverproperties) | Supported properties import read-only into the single TOML configuration, explicit TOML leaves win; unsupported keys warn. This is a documented subset, not every vanilla property or automatic world migration. |
| Ops, whitelist, player/IP bans, kick and access-list persistence | `partial` | [operator/access-control contract](OPERATING.md#server-console-and-access-control); [vanilla JSON regression coverage](../crates/mc-edge/src/access_compat_tests.rs); [login admission](../crates/mc-edge/src/login.rs) | Vanilla JSON import/persistence, operator whitelist exemption and fail-closed enforcement state. Full live access-control matrix and integrated [AUTH-01]/[AUTH-02] lifecycle/admission fixes remain separate acceptance requirements. |
| RCON and UDP Query | `partial` | [remote service lifecycle](../crates/mc-edge/src/remote_admin.rs); [RCON regressions](../crates/mc-edge/src/rcon_tests.rs); [Query regressions](../crates/mc-edge/src/query_tests.rs); [operating contract](OPERATING.md#rcon-and-udp-query) | Off-by-default authenticated TCP RCON through console and GameSpy4 UDP basic/full stat. External-client interoperability gate remains unrun; asynchronous plugin/backup completion is not a synchronous RCON receipt. Query has bounded requests and a five-second full-stat cache. |
| Native Wasmtime plugins, commands, hooks, permissions and durable KV | `partial` | [versioned deployed WIT contract](PLUGINS.md#L1-L8); [hooks/custody](PLUGINS.md#L51-L109); [plugin host fixes][plugins-fix] | Unsupported hooks (including before-damage) refuse startup; no SIGHUP reload. Direct block zone admission/storage-image size gaps ([Plugin SP-04], [Plugin SP-05]). A pending ABI bump is not tested compatibility; rebuild guests for the deployed WIT version. |
| Bukkit/Spigot/Paper JAR plugin compatibility | `missing` | [component-only deployment](PLUGINS.md#L3-L8), [package format](PLUGINS.md#L27-L32) | Wasmtime components do not execute JVM/Bukkit plugins; no API-compatibility claim for existing JARs. No audit finding ID assigned. |
| Solaris settlements, optional Loader and Distant Horizons | `n/a` | [settlements guide](SETTLEMENTS_GUIDE.md); [Loader](SOLARIS_LOADER.md); **tested with real client — DH Fabric**, recorded in [baseline matrix](https://github.com/kaiserproger/solaris/blob/0a86cfff/docs/COMPATIBILITY.md#distant-horizons); [graphical gate contract](OPERATING.md#L299-L328) | Extensions are not vanilla completion. Baseline records MC 26.1.2 + DH 3.3.3 cold far-LOD/live-geometry pixel pass; no new DH receipt inspected here, NeoForge untested. Material-only parent-LOD swaps remain an upstream limitation. |

## Real-client evidence

- **RC1 — tested with real client, agent-run native Java client.** Retained
  [result.json](../.analysis/codex-logs/FixP0Blocks/ground-after-result.json)
  (`client: java`, `profile: survival`, `status: passed`) and
  [diagnostic report](../.analysis/codex-logs/FixP0Blocks/fixes-report.md#L71-L103),
  run `20261023T044801-survival-p0palge_`, seed 81, server SHA-256
  `daa038d3e5f64eef21b5eb9d355d6946b407cd386c54b0cbf1d9814a4a451423`.
  Thirteen assertions cover joining, ordinary grass mining, authoritative
  removal, dirt pickup, selection, placement and item debit. Correct hand-mining
  rate was measured as 1/18 per tick. This is a worker-build receipt, not a fresh
  integrated-tree replay, full survival completion, combat gate or visual audit
  of every row that cites it.
- **RC-H — tested with real client, historical runtime only.** The retained
  [shared-chest routes](real-client-regression/README.md#L154-L172) name passing
  artifacts `20260622T221125Z-m94-regression-pack` and
  `20260622T232643Z-m94-regression-pack`. Their narrow item visibility/open-menu
  convergence is historical; [the runtime-era warning](real-client-regression/README.md#L8-L13)
  prohibits treating it as current native acceptance.
- **RC-G — gate exists, not a current pass claim.** The approved pack contains
  [water-bucket/solid/sign/recipe probes](real-client-regression/README.md#L117-L153)
  and [death/respawn/restart/two-client routes](real-client-regression/README.md#L173-L204).
  Broad `m94-02` through `m94-07` scenarios are explicitly blocked until their
  missing primitives/evidence exist. A manifest, observed DeathScreen or single
  recipe probe does not turn the broad feature row green.

No new client, server, build or gameplay test was run for this documentation
review. Future receipts must record binary/tree identity, exact scenario,
assertions and artifacts before promoting rows.

## Audit provenance and superseded findings

Audit IDs below link to retained **local, ignored** reports under `.analysis/`.
They resolve in the integrated evidence checkout, but are not shipped by a
normal Git clone. The matrix states the practical gap inline so a reader does
not need private artifacts to understand the status. Those reports' source line
numbers describe their audit tree, not every later refactor. A link is a finding
reference, not a claim that its repro was executed or every proposed fix landed.

| Audit source | Scope / how used |
|---|---|
| [VanCombat][combat-audit] | VC-01..20: combat and mob parity gaps; bounded positive source checks. |
| [VanItems][items-audit] | VI-01..14: item/station/container gaps; **VI-01 is superseded** by timed-mining fixes and RC1. |
| [VanBlocks][blocks-audit] | VB-01..30: blocks/redstone/fluids/light; **VB-01 is superseded** by protected-zone piston admission. |
| [VanSurvival][survival-audit] | VS-01..21: survival, modes, weather and respawn gaps. |
| [VanWorld][world-audit] | W01..13: fresh generation, portals and Anvil boundaries. |
| [SecAuth][auth-audit] | AUTH-01..09: session/admission/password-plugin lock gaps. Optional lock also has position/inventory disclosure and plate activation findings; online Mojang identity verification itself was checked fail-closed. |
| [SecCommands][commands-audit] | SC-03/05/06 remain cited gaps. SC-01/02/04 have [focused fixes][commands-fix] (spam, XP overflow, mute command admission). |
| [SecProtocol][protocol-audit] | Protocol SP-03/04/05 remain audit gaps. SP-01/02 have [focused fixes][protocol-fix]; current listener policy is documented in [OPERATING](OPERATING.md#L452-L460), not inferred from an older worker patch. |
| [SecPlugins][plugins-audit] | Plugin SP-04/05 remain audit gaps. SP-01/02/03 have [focused host fixes][plugins-fix] for config escape and permission publication/retirement. IDs overlap SecProtocol; prefixes here disambiguate them. |
| [IntegrityDupes][integrity-audit] | INT-01..05 have [focused fixes][integrity-fix] and current [custody rules](ARCHITECTURE.md#L81-L88); do not list the old chest/allay/round-trip/cursor/dead-pickup exploits as unfixed. |

The original [audit summary](../.analysis/codex-logs/Audit-SUMMARY.md) is a
historical inventory, **not a current open-defect count**. For fixed mining and
piston admission, see [mining corrections/RC1][mining-fix],
[mining regression tests](../crates/mc-core/src/tick/mining_tests.rs), and
[piston protected-footprint regression:48](../crates/mc-core/src/redstone/piston_tests.rs#L48).
For families with no assigned finding, `partial` explicitly records the evidence
limit instead of inventing an audit ID or claiming full compatibility.

[combat-audit]: ../.analysis/codex-logs/Audit-VanCombat/report.md
[items-audit]: ../.analysis/codex-logs/Audit-VanItems/report.md
[blocks-audit]: ../.analysis/codex-logs/Audit-VanBlocks/report.md
[survival-audit]: ../.analysis/codex-logs/Audit-VanSurvival/report.md
[world-audit]: ../.analysis/codex-logs/Audit-VanWorld/report.md
[auth-audit]: ../.analysis/codex-logs/Audit-SecAuth/report.md
[commands-audit]: ../.analysis/codex-logs/Audit-SecCommands/report.md
[protocol-audit]: ../.analysis/codex-logs/Audit-SecProtocol/report.md
[plugins-audit]: ../.analysis/codex-logs/Audit-SecPlugins/report.md
[integrity-audit]: ../.analysis/codex-logs/Audit-IntegrityDupes/report.md
[commands-fix]: ../.analysis/codex-logs/FixP0Commands/report.md
[protocol-fix]: ../.analysis/codex-logs/FixP0Protocol/report.md
[plugins-fix]: ../.analysis/codex-logs/FixP0Plugins/report.md
[integrity-fix]: ../.analysis/codex-logs/FixP0Dupes/report.md
[mining-fix]: ../.analysis/codex-logs/FixP0Blocks/fixes-report.md
[AUTH-01]: ../.analysis/codex-logs/Audit-SecAuth/report.md#L11
[AUTH-02]: ../.analysis/codex-logs/Audit-SecAuth/report.md#L27
[AUTH-03]: ../.analysis/codex-logs/Audit-SecAuth/report.md#L42
[Plugin SP-04]: ../.analysis/codex-logs/Audit-SecPlugins/report.md#L70
[Plugin SP-05]: ../.analysis/codex-logs/Audit-SecPlugins/report.md#L89
[Protocol SP-04]: ../.analysis/codex-logs/Audit-SecProtocol/report.md#L70
[Protocol SP-05]: ../.analysis/codex-logs/Audit-SecProtocol/report.md#L87
[SC-03]: ../.analysis/codex-logs/Audit-SecCommands/report.md#L27
[SC-05]: ../.analysis/codex-logs/Audit-SecCommands/report.md#L41
[SC-06]: ../.analysis/codex-logs/Audit-SecCommands/report.md#L48
[VB-02]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L18
[VB-03]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L24
[VB-04]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L30
[VB-05]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L36
[VB-06]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L42
[VB-07]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L48
[VB-08]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L54
[VB-09]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L60
[VB-10]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L66
[VB-11]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L72
[VB-12]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L78
[VB-13]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L84
[VB-14]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L90
[VB-15]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L96
[VB-16]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L102
[VB-17]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L108
[VB-18]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L114
[VB-19]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L120
[VB-20]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L126
[VB-21]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L132
[VB-22]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L138
[VB-23]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L144
[VB-24]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L150
[VB-25]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L156
[VB-26]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L162
[VB-27]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L168
[VB-28]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L174
[VB-29]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L180
[VB-30]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L188
[VC-01]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L14
[VC-02]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L24
[VC-03]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L34
[VC-04]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L44
[VC-05]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L54
[VC-06]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L64
[VC-07]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L74
[VC-08]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L84
[VC-09]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L94
[VC-10]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L104
[VC-11]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L114
[VC-12]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L124
[VC-13]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L134
[VC-14]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L144
[VC-15]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L154
[VC-16]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L164
[VC-17]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L174
[VC-18]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L186
[VC-19]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L196
[VC-20]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L206
[VI-02]: ../.analysis/codex-logs/Audit-VanItems/report.md#L20
[VI-03]: ../.analysis/codex-logs/Audit-VanItems/report.md#L27
[VI-04]: ../.analysis/codex-logs/Audit-VanItems/report.md#L34
[VI-05]: ../.analysis/codex-logs/Audit-VanItems/report.md#L41
[VI-06]: ../.analysis/codex-logs/Audit-VanItems/report.md#L48
[VI-07]: ../.analysis/codex-logs/Audit-VanItems/report.md#L55
[VI-08]: ../.analysis/codex-logs/Audit-VanItems/report.md#L62
[VI-09]: ../.analysis/codex-logs/Audit-VanItems/report.md#L69
[VI-10]: ../.analysis/codex-logs/Audit-VanItems/report.md#L76
[VI-11]: ../.analysis/codex-logs/Audit-VanItems/report.md#L83
[VI-12]: ../.analysis/codex-logs/Audit-VanItems/report.md#L90
[VI-13]: ../.analysis/codex-logs/Audit-VanItems/report.md#L97
[VI-14]: ../.analysis/codex-logs/Audit-VanItems/report.md#L104
[VS-01]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L15
[VS-02]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L22
[VS-03]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L29
[VS-04]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L36
[VS-05]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L43
[VS-06]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L50
[VS-07]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L57
[VS-08]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L64
[VS-09]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L71
[VS-10]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L78
[VS-11]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L85
[VS-12]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L92
[VS-13]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L99
[VS-14]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L106
[VS-15]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L113
[VS-16]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L120
[VS-17]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L129
[VS-18]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L136
[VS-19]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L143
[VS-20]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L150
[VS-21]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L157
[W01]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L13
[W02]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L21
[W03]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L29
[W04]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L37
[W05]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L45
[W06]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L53
[W07]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L61
[W08]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L69
[W09]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L77
[W10]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L85
[W11]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L93
[W12]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L101
[W13]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L109
