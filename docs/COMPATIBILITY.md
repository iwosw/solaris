# Solaris vanilla-mechanics compatibility (Minecraft Java 26.1.2)

Draft prerelease matrix. Status per row: `implemented+tested` | `implemented` |
`partial` | `unsupported` | `unknown`, with a one-line note and an evidence link.
Derived from the command registry, crates and docs; where the tree gives no
evidence the row says `unknown`. Not a release claim; see
[DEFINITION_OF_DONE.md](DEFINITION_OF_DONE.md) and the
[validation ledger](VALIDATION_LEDGER.md).

## Movement and physics

| Area | Status | Note | Evidence |
|---|---|---|---|
| Overworld collision, fall damage | `partial` | Entity collision model with tests; player fall fix in gate | [entity_collision_26_1_2.rs](../crates/mc-physics/src/entity_collision_26_1_2.rs), [physics-oracles.md](physics-oracles.md) |
| Sprint, diagonal speed, attributes | `implemented+tested` | Player diagonal-sprint gate passes on both sides | [QUIRK_PARITY.md](QUIRK_PARITY.md) |
| Water movement, swim feel | `partial` | Accepted placement/pickup proven; swim feel explicitly degraded | [real-client-regression/README.md](real-client-regression/README.md) |

## Blocks and redstone

| Area | Status | Note | Evidence |
|---|---|---|---|
| Pistons, QC, BUD, TNT duping, zero-tick | `partial` | QC/TNT/zero-tick gate passes; instant piston move, no `moving_piston` dupe window | [QUIRK_PARITY.md](QUIRK_PARITY.md) |
| Dispenser/dropper QC | `partial` | Fixed with regression tests; no oracle scenario yet | [QUIRK_PARITY.md](QUIRK_PARITY.md) |
| Falling blocks (sand etc.) | `partial` | Landing/removal path covered; broad variants incomplete | [VALIDATION_LEDGER.md](VALIDATION_LEDGER.md) |
| Doors, trapdoors, buckets, block use | `partial` | Focused oak/copper/iron and bucket probes; broad use unproven | [VALIDATION_LEDGER.md](VALIDATION_LEDGER.md) |
| Command blocks | `unsupported` | No command-block execution path in the tree | [command_tree.rs](../crates/mc-edge/src/command_tree.rs) |

## Fluids

| Area | Status | Note | Evidence |
|---|---|---|---|
| Water/lava spread, ticks | `partial` | Scheduled-tick model with save/restart delay accounting | [fluid_parity_tests.rs](../crates/mc-core/src/fluid_parity_tests.rs), [VALIDATION_LEDGER.md](VALIDATION_LEDGER.md) |
| Buckets, waterlogging, lava-water interaction | `partial` | Focused bucket probes pass; broad spread/interaction degraded | [real-client-regression/README.md](real-client-regression/README.md) |

## Crafting, smelting, inventory, containers

| Area | Status | Note | Evidence |
|---|---|---|---|
| Crafting (shaped/shapeless), recipe book | `partial` | One shapeless oak-log→planks probe via real client; broad recipes pending | [real-client-regression/README.md](real-client-regression/README.md) |
| Furnace family, campfire, brewing, stations | `partial` | Furnace/campfire/brewing layouts with TCP coverage; not full parity | [VALIDATION_LEDGER.md](VALIDATION_LEDGER.md) |
| Chests, hoppers, containers, QuickCraft | `partial` | Chest left/right-drag, hopper transfers, shared-chest two-client probes | [VALIDATION_LEDGER.md](VALIDATION_LEDGER.md), [real-client-regression/README.md](real-client-regression/README.md) |
| Signs, beds, campfires as block entities | `partial` | Regular-sign real-client probe passes; hanging/waxed/beds pending | [real-client-regression/README.md](real-client-regression/README.md) |

## Mobs and AI

| Area | Status | Note | Evidence |
|---|---|---|---|
| Spawning, despawn, mob caps | `partial` | Core spawn paths work; broad AI/pathfinding stays draft | [VALIDATION_LEDGER.md](VALIDATION_LEDGER.md) |
| Hostile AI, pathfinding, farm rates | `partial` | Farm kill-chamber drift known (skeleton wanders off rim) | [QUIRK_PARITY.md](QUIRK_PARITY.md) |
| Fall damage, loot tables | `implemented+tested` | Mob fall accumulation and golem loot fixed with oracle runs | [QUIRK_PARITY.md](QUIRK_PARITY.md) |

## Combat

| Area | Status | Note | Evidence |
|---|---|---|---|
| Melee damage, crit, knockback, sweep | `partial` | Native melee model with unit tests; broad real-client proof pending | [combat_tests.rs](../crates/mc-core/src/combat_tests.rs) |
| Projectiles, shields, XP pickup | `partial` | Arrow/shield/deflection unit paths; hostile combat blocked on client primitives | [real-client-regression/README.md](real-client-regression/README.md) |
| Death, respawn, drops | `partial` | DeathScreen + respawn observed once; broad combat drops pending | [real-client-regression/README.md](real-client-regression/README.md) |

## Farming and growth

| Area | Status | Note | Evidence |
|---|---|---|---|
| Crops, sugar cane, cactus, bamboo | `implemented+tested` | Growth-rate fixes proven by farm-rate oracle runs | [QUIRK_PARITY.md](QUIRK_PARITY.md) |
| Chicken eggs, iron farms | `implemented+tested` | Egg RNG and golem loot fixes pass oracle comparison | [QUIRK_PARITY.md](QUIRK_PARITY.md) |
| Villager breeder rate | `partial` | Known divergence: farmer harvest ~2.4x vanilla | [QUIRK_PARITY.md](QUIRK_PARITY.md) |

## Villagers and trading

| Area | Status | Note | Evidence |
|---|---|---|---|
| Workstations, jobs, restocking | `partial` | Day-tick allocator with restock tests; bound-resident rules are Solaris-specific | [SETTLEMENTS_GUIDE.md](SETTLEMENTS_GUIDE.md) |
| Trading UI, emerald economy | `partial` | Native offers with settlement escrow; broad haggling/demand unproven | [SETTLEMENTS_GUIDE.md](SETTLEMENTS_GUIDE.md) |

## Worldgen and structures

| Area | Status | Note | Evidence |
|---|---|---|---|
| Overworld terrain, biomes | `partial` | Deterministic generator; vanilla-like, not bit-exact | [terrain.rs](../crates/mc-worldgen/src/terrain.rs), [VILLAGE_GENERATION.md](VILLAGE_GENERATION.md) |
| Nether, End generation | `partial` | Dedicated generators; broad decoration parity unproven | [nether.rs](../crates/mc-worldgen/src/nether.rs), [end.rs](../crates/mc-worldgen/src/end.rs) |
| Structures, `/locate` | `partial` | Sidecar-backed structure ids for suggestions; full placement parity unproven | [structures.rs](../crates/mc-worldgen/src/structures.rs) |

## Dimensions

| Area | Status | Note | Evidence |
|---|---|---|---|
| Overworld/Nether/End travel, portals | `implemented` | Dimension travel, portal rules and gamerule fencing with tests | [dimensions_tests.rs](../crates/mc-core/src/dimensions_tests.rs) |
| End fight, gateways, dragon egg | `implemented` | Native end-fight state machine with pillar/gateway logic | [end_fight.rs](../crates/mc-core/src/end_fight.rs) |
| Beds, anchors, respawn rules | `implemented` | Bed/anchor dimension rules enforced natively | [dimensions_tests.rs](../crates/mc-core/src/dimensions_tests.rs) |

## Commands

Advertised roots come from [command_tree.rs](../crates/mc-edge/src/command_tree.rs)
(decompiled 26.1.2 command sources cited in its header). `/settlement`, `/tps`,
`/mspt`, `/co`, `/mute`, `/unmute`, `/warn` are Solaris additions.

| Command | Status | Note |
|---|---|---|
| `/help`, `/list`, `/me`, `/msg`, `/tell`, `/w` | `implemented` | Social basics; `/teammsg` missing |
| `/gamemode`, `/teleport`, `/tp`, `/execute` | `implemented` | Core admin/movement stack |
| `/give`, `/clear`, `/kill`, `/summon` | `implemented` | Spawn/item plumbing incl. villager/item NBT fixes |
| `/time`, `/weather`, `/difficulty`, `/gamerule` | `implemented` | Clock/weather/rule authority |
| `/setblock`, `/fill`, `/forceload`, `/worldborder` | `implemented` | Edit/ticket/border paths |
| `/experience`, `/xp`, `/enchant`, `/effect` | `implemented` | XP levels and total points saturate at `i32::MAX`; full XP bars at the level ceiling consume points without wrapping the level |
| `/spawnpoint`, `/setworldspawn`, `/seed` | `implemented` | Spawn/seed reads |
| `/say`, `/tellraw`, `/title`, `/bossbar` | `implemented` | Broadcast/presentation (bossbars session-kept) |
| `/advancement`, `/team`, `/scoreboard`, `/locate` | `implemented` | Meta/progress/lookup |
| `/tick`, `/save-all`, `/stop` | `implemented` | Freeze/sprint/step, checkpoint, halt |
| `/kick`, `/ban`, `/ban-ip`, `/pardon`, `/pardon-ip` | `implemented` | Moderation set |
| `/op`, `/deop`, `/whitelist`, `/banlist`, `/transfer` | `implemented` | Access-list and transfer plumbing |
| `/mute`, `/unmute`, `/warn`, `/co`, `/tps`, `/mspt` | `implemented` | Solaris moderation/diagnostics extras |
| `/settlement` (+ `guide/menu/build/production/commerce/payroll/caravan/army`) | `implemented` | Solaris town game loop |
| `/debug give` | `implemented` | Retained Solaris test fixture, not vanilla |
| `/clone` | `unsupported` | No region-copy command |
| `/data` | `unsupported` | No NBT inspect/mutate command |
| `/function`, `/datapack`, `/schedule` | `unsupported` | No function/datapack runtime or scheduler |
| `/place`, `/fillbiome` | `unsupported` | No feature/biome placement commands |
| `/damage`, `/attribute`, `/loot`, `/item`, `/recipe`, `/trigger`, `/tag` | `unsupported` | No entity/item/recipe/tag manipulation commands |
| `/spreadplayers`, `/rotate` | `unsupported` | No scatter/facing helpers |
| Command blocks (impulse/chain/repeat) | `unsupported` | No block-driven command execution |

## Chat, auth, multiplayer

| Area | Status | Note | Evidence |
|---|---|---|---|
| Chat, tells, server messages | `implemented` | Styled chat/tell pipeline with tests | [command_tree.rs](../crates/mc-edge/src/command_tree.rs) |
| Offline mode, Mojang auth, Velocity forwarding | `implemented` | Offline default; online `hasJoined` + Velocity verify paths | [login.rs](../crates/mc-edge/src/login.rs) |
| Whitelist, ops, two-client visibility | `partial` | Access lists plus focused two-client shared-chest/drop probes | [real-client-regression/README.md](real-client-regression/README.md) |

## Persistence and saves

| Area | Status | Note | Evidence |
|---|---|---|---|
| Native store + tick WAL + checkpoints | `implemented+tested` | `native/state.redb` serving truth; crash tail rolls back consistently | [native_checkpoint.rs](../crates/mc-core/src/native_checkpoint.rs), [OPERATING.md](OPERATING.md) |
| Anvil import/export | `implemented` | Explicit offline conversion with receipts; never a live fallback | [OPERATING.md](OPERATING.md) |
| Save/restart of players, chunks, entities, time/weather | `partial` | Focused restart gates pass; broad soak/crash-matrix unproven | [VALIDATION_LEDGER.md](VALIDATION_LEDGER.md) |

## Plugins (Wasm)

| Area | Status | Note | Evidence |
|---|---|---|---|
| Wasmtime components, `solaris:plugin@0.7.0` | `partial` | Directory deployment, manifests, bounded ops; no SIGHUP reload | [PLUGINS.md](PLUGINS.md), [lib.rs](../crates/mc-plugin-host/src/lib.rs) |
| `before-build` hooks, plugin KV | `partial` | Covered hook set with fenced approvals; unsupported hooks fail startup | [PLUGINS.md](PLUGINS.md) |

## Distant Horizons

| Area | Status | Note | Evidence |
|---|---|---|---|
| Distant Horizons mod support | `implemented+tested` | Protocol 16, MC 26.1.2 + DH 3.3.3. Fabric: real-client gate passed (cold far LOD of a never-visited landmark + live geometry edit pixel-confirmed). NeoForge not tested. Known upstream limitation: material-only swaps with unchanged geometry do not propagate to parent LODs (DH compares raw ids). | [dh.rs](../crates/mc-edge/src/dh.rs), [OPERATING.md](OPERATING.md#distant-horizons) |

## Open questions (`unknown`)

* Exact vanilla command surface beyond the rows above (no full 26.1.2 matrix run yet).
* Raid, trial-chamber/vault loop and boss-bar `custom_boss_events.dat` export depth.
* Performance headroom vs the 20-player view-distance-8 target on a 4-vCPU VPS.
