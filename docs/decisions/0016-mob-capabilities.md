# ADR 0016 - Mob capabilities and data-defined blueprints

**Date:** 2026-10-23

**Status:** Accepted. Layer 1 implemented. Layer 2 (registration through plugins and
data files) is next. Sensors step 1 is implemented. Layer 3 (Wasm capability code)
and persistent sensor subscriptions are proposed only.

## Context

ADR 0013 ("Mob blueprints") says a mob type is a blueprint: movement class, goals,
sensors and parameters. It also says there is "no per-type code path a plugin type
cannot use". The core did not fully meet that. Blueprints were `'static` Rust
tables, and many shared behaviours were decided by comparing type names or type
ids (`is_enderman()`, `Some(type_id) == *X_TYPE`, `matches!(name, "minecraft:…")`).
The appendix lists 124 function-level sites (531 lines in 56 files at
`286c275c`). The owner wants players and addons to compose mobs that are not tied
to a vanilla type, for example a mob that is hurt by water, explodes like a
creeper and flies.

## Decision

### Capability model

`MobBlueprint` (`crates/mc-core/src/blueprint.rs`) is owned data
(`name: String`, `goals: Vec<Goal>`). It holds:

- **movement class**: `Walker | Swimmer | Flyer | Amphibious | Stationary`. *Flies*
  is `movement = Flyer`: three-dimensional steering with no gravity. It is not a
  separate flag.
- **goals**: the existing `GoalKind`s with priorities.
- **sensors**: `sight_range`, `attack_reach`.
- **params**: `max_health`, `speed`, `attack_damage`, plus the new `armor` and
  `knockback_resistance`.
- **`capabilities: Capabilities`**: shared behaviours. Exactly one generic system
  runs each one for every blueprint that sets it.

| Capability | Executor | Vanilla users (built-in table) |
| --- | --- | --- |
| `burns_in_daylight` | `natural_spawn` sun ignition | zombie, zombie_villager, skeleton, stray, bogged, parched, phantom |
| `night_aggro_only` | `tick::read_row` target gate | spider, cave_spider |
| `hurt_by_water` | `contact_hazards::hurt_by_water` (1 drowning damage per tick in water or rain) | enderman (vanilla also blaze, snow_golem, strider; see below) |
| `fall_damage_immune` | `mob_fall::after_move` | 18 types of `#minecraft:fall_damage_immune` |
| `climbs_walls` | `entity_motion::step` (0.2 blocks per tick) | spider, cave_spider |
| `inverted_healing` | potion payloads (instant health/damage swap) | 14 undead |
| `aquatic` (`Fish`/`Squid`/`Dolphin`/`Other`) | `entity_motion::step` | cod, salmon, tropical_fish, pufferfish, tadpole; squid, glow_squid; dolphin |
| `explodes {power, fuse_ticks, griefing}` | `hostile::swell` (the `Swell` goal) | creeper (3.0, 30, true) |
| params `armor` / `knockback_resistance` | `entity_loot::restore` | zombie family 2.0; iron_golem 1.0, hoglin/zoglin 0.6, ravager 0.75 |
| `temptable` | `tick::passive_follow` reads the existing resolved `breed_food` and both player hands | built-in breeding animals |
| `follows_adults` | `tick::passive_follow` queries existing chunk membership for nearby same-blueprint adults | built-in breeding animals |
| `crossbreed {partner, offspring}` | `entity_life::breeding` resolves reciprocal parent pairs; merge revalidates readiness, distance and child type | horse + donkey -> mule |

The original layer-1 capabilities reused working code. No capability was invented
without one: there is no `fire_immune`, because `mc-core` never consumed the data
flag `EntityTypeFlags::FIRE_IMMUNE`. Built-in vanilla behaviour is unchanged.
The type lists moved, unedited, into one function, `blueprint::vanilla_capabilities`.
Vanilla blaze, snow golem and strider are also water-sensitive, but they did not
take water damage before this change. Turning on `hurt_by_water` for them is a
separate parity change.

Food temptation and juvenile following are native shared capabilities added for
VC-06. They run on the existing AI decision cadence, below panic, leash and active
courtship, using immutable player inventory and entity positions. Player searches
share one target grid; juvenile searches visit only the chunks intersecting an
eight-block radius and allocate nothing. Sitting, riding and NoAI keep their
existing suppression. Rest proofs wake on decision ticks when held items or a
juvenile could create a new target, including within-chunk changes.
These goals use the existing collision-aware direct steering, not a promise of
full obstacle navigation. `temptable` needs resolved `breed_food`; it does not
implicitly inherit food from a custom mob's presentation type.

Crossbreeding is a reciprocal data relation, not a horse type check in the tick.
Both parents must name each other's blueprint key and the same registered child;
same-blueprint pairs retain their ordinary `offspring`. Merge checks the pair
again and refuses self-pairing, an incorrect child, lost eligibility or separation
of three blocks or more before cooldowns or spawn custody change. Horse/donkey
rules are assembled once with registry-resolved ids; mule sterility is unchanged.

Logic unique to one boss or special mob stays type-keyed on purpose. This covers
enderman stare anger and teleport dodging, the dragon fight, the wither, warden,
raids, piglin bartering, bees, allays, copper golems, creaking, villagers and
dolphin toys. The appendix gives the reason for each. Their dedicated attack goals
(`BlazeBurst`, `SiegeShot`, ... `VexLife`) keep per-type retained state, so data
blueprints cannot use them.

### Identity and client presentation

The protocol cannot add entity types. A data-defined blueprint therefore names a
vanilla living type in `presents_as`. Its key is
`CUSTOM_TYPE_FLAG | ordinal << 16 | wire_type_id`. All tick behaviour reads the
blueprint through that key (`CoreRegistry::blueprint`, O(1) dense index). Name
checks for vanilla-specific behaviours never match a custom key, so a custom mob
does not inherit, for example, bogged poison arrows. `blueprint::wire_type_id(key)`
maps the key back to the presentation type. `EntityStore::wire_view` sends that
type to clients, and `contact_hazards` uses it to decide whether the mob is living.

Limits that follow from presenting as a vanilla type:

- Clients render, animate and play the sounds of the presentation type. A flying
  "creeper" shows the creeper model and walk animation in mid-air.
- The body dimensions come from the presentation type, so server and client
  hitboxes agree.
- Entity data is sent only when it fits the presentation type's wire layout. For
  example, the swell fuse metadata (`CREEPER_SWELL_DIR_INDEX`) is written only when
  the mob presents as a creeper; any other presentation keeps the fuse on the
  server. Writing a creeper field into another type's layout would break client
  decoding.
- Vanilla kill and riding statistics and plugin event `entity_type` names are
  vanilla-typed. A custom key reports none of them; it does not report the
  presentation type's.

### Validation: bounded, fail closed

`MobBlueprintDef::admit` turns a JSON definition into a blueprint. Any field
outside its bound is a load error that names the field. Nothing is clamped, and a
rejected definition adds nothing to the registry.

| Field | Bound | What it bounds |
| --- | --- | --- |
| data-defined blueprints per registry | ≤ 64 | registry size and dense index |
| `goals` | 1..=8, priority 0..=15, no duplicates, only `flee_fire, melee_chase, melee_attack, wander, swim, idle, ranged_attack, swell` | per-mob goal evaluation |
| `max_health` | 1..=1024 | vanilla attribute ceiling |
| `speed` | 0..=16 blocks/s (0.8/tick) | swept-collision length per tick |
| `attack_damage` | 0..=100 | gameplay sanity (warden 30) |
| `sight_range` | 0..=64 blocks | target search: at most 9×9 target-grid cells per query |
| `attack_reach` | 0..=24 and ≤ `sight_range` | ranged/fuse start distance |
| `armor` / `knockback_resistance` | 0..=30 / 0..=1 | vanilla attribute ranges |
| `explodes.power` | 0.5..=6.0 (charged creeper) | 1352 rays × at most about 35 steps; entity radius 2 × power |
| `explodes.fuse_ticks` | 10..=600 | at most one blast per exploder per 10 ticks; the exploder dies |
| `swell` ⇔ `explodes`; `swell` excludes `ranged_attack` | — | each goal has the data its executor needs |
| `presents_as` | vanilla `Living` contract, not `player`, not `ender_dragon` | client decoding; the client dragon creates eight part entities at wire ids parent+1..=8, and only `end_fight` reserves them |
| `presents_as` body | half width ≤ 2.0, height ≤ 6.0 (the giant, height 12, fails) | the contact-hazard scan envelope (`contact_cells` clamps there), swept-collision cells and spawn-clearance checks per mob |
| `movement` | not `item`, `player` or `projectile` | — |
| `category` | vanilla `MobCategory` name | despawn rules |
| `loot_table` | must exist in the vanilla entity loot catalog | — |
| `id` | not in the `minecraft` namespace, ≤ 128 bytes, unique | — |

Data-defined blueprints get `spawn_weight = 0`: no natural spawning in layer 1.

### Data sources

- **Built-in vanilla:** Rust (`vanilla_blueprints` plus `vanilla_capabilities`) is
  the default registry. It is not loaded from files.
- **External:** JSON in the `MobBlueprintDef` shape. The repository already uses
  JSON for content: settlement blueprints in `assets/blueprints/*.json`, and the
  plugin startup contribution's work recipes and block tools as JSON strings.
  `MobBlueprintDef::from_json` plus `CoreRegistry::register_blueprint` is the only
  admission path.
- **Layer 2:** two sources, one validator. (1) Plugins: add
  `mob-blueprints: option<list<string>>` to the WIT `startup-contribution`, beside
  `block-tools`, and admit them in `plugin_startup::apply` under the existing
  package admission. (2) An operator data directory, read at `Core::open` before
  world admission. The startup admission must order definitions deterministically
  (sorted by id). It must also add the admitted definitions to
  `plugin_startup::world_identity`, so reopening a world with a different blueprint
  set fails closed instead of remapping persisted keys. Neither source adds a
  runtime: blueprints are data, and spawns enter as intents through the existing
  admission.

### Cost accounting

Native capabilities are **not free and not sandboxed**. They run in the core tick
on core CPU, and plugin guest fuel does not meter them. A badly tuned definition
slows the whole tick, not only its own mobs. Fuel limits guest instructions inside
one plugin callback; it does not protect the tick. A data-defined mob costs the
same as a vanilla mob with the same goals and capabilities, because no new
executor exists. The bounds above cap the cost per mob and per blast. The blast
entity pass in `explosion::hurt_entities` is O(entities in the world) per blast,
the same as for vanilla creepers today. The fuse floor and population limits
bound it.

Population is bounded only by existing paths. Data-defined types never spawn
naturally (weight 0). Plugin `SpawnEntity` goes through the existing command
admission: `PluginLimits::commands_per_call` (32 per callback) and the
`MAX_SPAWN_ENTITY_TYPES` allow-list (32). Today **no per-plugin live-population
cap exists**. Before layer 2 accepts plugin-defined types, it must add one, for
example a maximum number of live data-defined mobs per owning plugin, refused at
`SpawnEntity` admission. That cap is host-side accounting next to
`PluginLimits::hostcall_bytes`, not guest fuel.

### Layer 3 sketch (not implemented)

Layer 3 lets an addon add custom capability logic in Wasm. The design:

- Capability callbacks are event-driven and batched. Examples: "mob hurt", "target
  acquired", "fuse ended", or a capped interval tick. One batched callback per
  plugin per tick covers all its mobs; there is never one call per mob.
- The guest returns intents through the existing plugin → command → intent
  admission path. It never mutates mobs directly.
- `PluginLimits` bounds guest cost (`fuel_per_call`, `epoch_ticks_per_call`, guest
  memory). Host cost is accounted separately: batch size (`events_per_batch`),
  answer bytes (`hostcall_bytes`) and the intents admitted per callback
  (`commands_per_call`).
- Throttling: an owner that exceeds its budget gets fewer events per tick (skipped
  intervals) instead of stalling the tick. Native capabilities keep running.

### Sensors: batched broad-phase, goal-based retention

Step 1 uses the existing per-tick index, not a new sensor runtime. The radius
bounds above apply. `TargetGrid::batch` builds immutable candidate slices before
the worker join, at the largest requested radius in each consumer cell. Empty
classes allocate nothing. Small target tables are filtered once per cell rather
than first building a target index. There are no persistent wake subscriptions.

- **Step 1.** Batched broad-phase over the existing per-tick
  `target_grid::TargetGrid`. For each occupied 16-block cell, build one candidate
  list per target class per tick from the cells within the largest admitted radius
  (`sight_range` ≤ 64 means at most 9×9 cells). Each mob in that cell then makes
  exactly the same acquisition selection as before (`TargetGrid::nearest_within`):
  exact 3D distance ≤ range, caller acceptance, class chain order, table order on
  ties. Existing LOS checks stay at their original goal-specific stage; visibility
  is never cached per cell. Seeded batched/unbatched equivalence tests include
  negative cells, boundary radii, mixed ranges, per-mob visibility predicates and
  class ties. The fence applies to acquisition, not to intentional retention of
  a previously chosen target when another candidate becomes nearer.
- **Target retention by event.** A chosen target stays chosen until an event drops
  it: target died or despawned, target left `sight_range`, or the mob was hurt by
  another attacker (revenge). The retention test is an exact per-tick
  distance/liveness check on one target, not a search.
  This is transient `HostileState` memory for generic MeleeChase/RangedAttack/Swell
  goals; native special brains and enderman anger keep their existing owners.
  Committed damage pushes revenge, excluding creepers and slimes, which lack
  HurtByTargetGoal. Ranged/fuse executors use the same retained target rather
  than independently switching to a nearer player. Player eligibility/session
  changes, no-AI, peaceful and the existing night-only gate also invalidate
  memory. Player-created golems never retain a player. Missing targets are
  searched on ordinary decision cadence; same-cell movement needs no wake event.
- **Persistent wake subscriptions: later option only.** Here mobs with no
  candidate sleep until something wakes them. This is allowed only after a
  dense-combat measurement and a complete list of invalidation sources:
  - a target enters the radius *inside the same chunk or cell*
  - a target leaves the radius
  - LOS opens or closes after a block is broken or placed (explosions, pistons,
    falling blocks, doors)
  - a target spawns, despawns, dies or respawns
  - teleport (commands, pearls, chorus fruit)
  - dimension change
  - game mode or session lock changes
  - the subscriber's own movement
  - day/night flips for `night_aggro_only`
  - difficulty set to peaceful

  Subscriptions that fire only when a target crosses a chunk boundary would miss
  most of these. The main risk is a missed wake: a mob that ignores a player
  standing next to it. The guard is a seeded differential test that runs the same
  scenario (random walks, teleports, LOS block edits, spawns and despawns, game
  mode and dimension changes) with per-tick search and with subscriptions, and
  requires the same target per mob per tick.
- **Cost model.**
  - Quiet world, no player near: target tables are empty, so `has_live_targets`
    already skips specials and every chaser query hits an empty table. The
    broad-phase saves nothing. Remaining think cost is the AI cadence, which the
    rest/sleep work addresses, not sensors.
  - Dense combat, 20 players against 1000 mobs in about 4×4 chunks: the
    20-player table is below `MIN_INDEXED` (32), so the old path made **no cell
    walks**: it scanned the table per mob. Batched acquisition performs about
    16 broad phases, then still up to 20k exact checks. On subsequent ticks,
    retention replaces those searches with 1000 one-target validations. Quiet
    empty-table scenes have no broad phases to save. The ignored
    `sensor_bench_tests::sensors_release_entity_phase_dense_and_quiet` fixture
    measures the whole in-process entity phase on both revisions, with 120 warmup
    ticks and three samples of 600 ticks under an exclusive approved window.
    Debug-only query counters are absent from release timing.

Removal boundary: omit `TargetGrid::batch` to restore unbatched acquisition
without changing choices. Retention is a separate intentional gameplay change,
not a claim of equivalence with unconditional nearest-target switching.

## Consequences

- One behaviour has one executor and one data switch. A new vanilla mob that
  shares a behaviour sets a flag in `vanilla_capabilities` instead of adding a
  type check.
- `CoreRegistry::blueprint` is O(1) through a dense index instead of a linear
  scan. Registry mutation goes through `register_blueprint` (and the test-only
  `retain_blueprints`), which rebuilds the index.
- Layer 2 must persist data-defined identities before it makes them reachable in
  production. Today `core.rs::entity_capture_record` and Anvil export name
  entities through the vanilla contract and would drop a custom key. The journal
  persists raw keys, which `world_identity` must pin.

## Appendix: type-decision inventory (`286c275c`)

Dispositions:

- **C**: converted to a capability or parameter in layer 1.
- **U**: unique boss or special mechanic, kept.
- **P**: bound to presentation, NBT or wire layout; correctly keyed on the vanilla
  type.
- **D**: shared family deferred to a later capability.

The full site list, one row per function, is in
`.analysis/codex-logs/Capabilities/inventory-sites.tsv` (ignored, local).
Non-mob entity kinds are excluded because they are entity-kind dispatch, not mob
behaviour: projectiles, vehicles, item, XP orb, TNT, falling block, leash knot,
end crystal and armor stand.

| Site | Behaviour | Shared by (vanilla) | Disp. |
| --- | --- | --- | --- |
| `blueprint.rs:667-683`, `species.rs:210-217` | sun burn and night-only aggro set by name | zombie, zombie_villager, skeleton, stray, bogged, parched, phantom; spider, cave_spider | C |
| `species.rs:223-229`; `tick.rs:5345,6673` | wall climbing | spider, cave_spider | C |
| `species.rs:231-253`; `tick.rs:5346,6674` | swimmer control family | 5 fish; squid, glow_squid; dolphin | C |
| `species.rs:255-277`; `entity_loot.rs:538-539` | natural armor, knockback resistance | 5 zombies; iron_golem, hoglin, zoglin, ravager | C |
| `mob_fall.rs:16-46,70-73` | fall-damage immunity | 18 types | C |
| `projectiles/payloads.rs:186-206` | inverted instant health/damage | 14 undead | C |
| `enderman.rs:13-18,193-223` | water/rain damage (`ENDERMAN_TYPE` lane) | enderman (vanilla: + blaze, snow_golem, strider) | C (damage); the teleport stays U |
| `hostile.rs:26-31,459-467` | swell fuse 30, power 3, creeper grief switch for every `Swell` | creeper | C |
| `enderman.rs:16-18,59-190,224-246`; `tick.rs:5737,5753`; `projectiles/embedded_projectiles.rs:45`; `journal.rs:2696` | stare anger (retained NBT), projectile dodge by teleport, chase only while angry, no arrow embedding, replay position | — | U |
| `end_fight.rs:246-705`, `end_fight/persistence.rs:244-313`, `world_conversion/import.rs:1793-1836`, `portals_travel.rs:56-59` | dragon fight and crystals; dragon and wither never use portals | wither (portal rule) | U |
| `special_mobs/wither.rs:37,211`; `survival.rs:121-140` | wither immunities, summoning, skull damage type | — | U |
| `special_mobs/blueprints.rs:12-38`, `special_mobs/summons.rs:235`, `special_mobs/persistence.rs:81-157` | dedicated attack goals and retained state | blaze, ghast, pillager, witch, breeze, guardians, warden, evoker, shulker, wither, vex | U (goal kinds are data; executors per type) |
| `special_mobs/raiders.rs:24-61,113` | raid roles | pillager, vindicator, evoker, illusioner, ravager, witch | U |
| `shrieker.rs:525`, `creaking.rs:10`, `entities.rs:1654` | warden summon; creaking heart, no motion rest | — | U |
| `bees.rs:13-1186`, `allay.rs:15-344`, `copper_golem.rs:16-672`, `dolphin.rs:18-297`, `tick.rs:6643` | hive, delivery, sorting, dolphin drive/toys | — | U |
| `piglin.rs:36-93`; `tick.rs:5598-5606,5744-5758,6464`; `entities.rs:1720-1735`; `tick.rs:6050-6057` | target classes: piglins, zombies hunt villagers, golems hunt monsters, zoglins, villager attackers; adult-only piglin chase | zombie+zombie_villager; iron_golem | D (target classes become sensor interests) |
| `villager/conversion.rs:21-77,195`, `villager/defense.rs:62-142`, `villager/soldier.rs:31-54`, `villager/brain.rs:2617`, `villager/shepherd.rs:37`, `tick/merchant.rs:25`, `mob_lanes.rs:44-251`, `plugin_npcs.rs:428`, `settlements/autonomy.rs:128` | villager life, zombie infection, golems ignore creepers, guards' hostile list, traders | 24 hostile types (guards) | U/D |
| `mob_lanes.rs:21,366`, `golems.rs:57-81` | snow golem trails; golem build patterns | iron/snow golem | U |
| `hostile.rs:399`, `entity_loot.rs:146`, `projectiles/payloads.rs:1043,1129`, `effects_trigger.rs:61,104` | bogged poison arrows, cave spider poison, snowballs hurt blaze, breeze ignores wind charges, infested/oozing spawns | — | D (on-hit effect, projectile vulnerability) |
| `entity_loot.rs:471-474,526-570,731`; `tick.rs:5745-5747`; `natural_spawn.rs:882-892` | slime size (entity data 16, `Size`), split on death, size-1 harmless; slime-chunk and nether light rules | slime, magma_cube; ghast | P (size) / D (split, spawn rules) |
| `blueprint.rs` wool, `settlements/construction_availability.rs:141`, `core.rs:2662-2722` | sheep wool data index 18, capture | — | P |
| `command_summon.rs:198-494`, `dimensions_recovery.rs:37-227`, `journal.rs:2162-2325`, `entities.rs:1548-1551`, `entity_life.rs:156,638-645`, `projectiles/payloads.rs:850`, `world_conversion/census.rs:262`, `villager/generated.rs:41` | summon NBT keys, retained NBT, egg/tadpole clocks, hatching | chicken, tadpole, villager, … | P |
| `statistics.rs:153-165,216`, `advancements.rs:231-239`, `plugin.rs:2443,2544` | vanilla stat ids, advancement predicates, plugin event names | 14 rideables | P |
| `taming.rs:56-396` | tame, sit, collar, inherit | cat, wolf, parrot, horses, llamas, fox, ocelot | D |
| `leash.rs:24-56` | leadable | 33 types | D |
| `mounts.rs:57-538` | rideable, saddle, steering item | pig, strider, camel, horses, llamas | D |
| `entity_life.rs:100-106,303-334,555,806-835`, `golden_dandelion.rs:43-76` | breeding gated by taming, milking, ageable list | 30 ageable species | D |
| `mob_equipment.rs:103-255`, `mob_equipment_pickup.rs:13-98` | spawn equipment, item pickup | zombies, skeletons, piglins, illagers, vex | D |
| `species.rs:105-217,321-441`, `blueprint.rs:348-677`, `registry.rs:420-426`, `trial_spawner.rs:119` | construction-time vanilla tables and defaults | — | data, already blueprint fields |
