# Migrating a vanilla or Paper 26.1.2 server

Solaris is a **draft prerelease**, not a drop-in Paper replacement. This guide
moves a stopped, ordinary Minecraft Java **26.1.2** world into Solaris's native
store and describes how to convert it back. Test on copies before changing the
public endpoint. A successful conversion receipt proves conversion, not complete
gameplay compatibility: consult [COMPATIBILITY.md](COMPATIBILITY.md).

The configuration and conversion behavior below was inspected at commit
`0a86cfffc173d9fe13e9d0795a6c5494fc33a222`. Recheck it against the binary you
deploy; draft configuration and storage contracts can change between builds.

## 1. Prerequisites and a recoverable source

- Use the `solaris-v2` binary from the same build throughout the rehearsal. Release
  packaging may name it `solaris`; substitute that name consistently. Confirm
  `solaris-v2 --help`, `solaris-v2 content import --help`,
  `solaris-v2 world import-anvil --help` and
  `solaris-v2 world export-anvil --help` on the actual installed binary.
- Install **JDK 25**, including `java` and `javac`, for the one-time content
  extraction and the vanilla rollback rehearsal. Have a licensed official
  **26.1.2 server bundle**, not a Paper launcher JAR. `JAVA`/`JAVAC` select the JDK.
  Review Minecraft's license/EULA yourself; this guide does not accept terms.
- Reserve disk space for the unchanged backup, a staging copy, the native world,
  a second Anvil export and conversion staging. There is no safe fixed size ratio.
  Do not use a live world's directory as a destination or run two writers on it.
- Record the actual numeric world seed (`seed` in the old server console), game
  version, enabled datapacks, dimension list, game rules, difficulty, world spawn,
  border, authentication/proxy mode and plugin versions. An empty or textual
  `level-seed` property is **not** the numeric seed of an existing world.
- Run `save-all flush`, then `stop` on vanilla/Paper and wait for process exit.
  Back up the **entire server installation**, including every world, config,
  access list, plugin JAR/config/database, resource pack and player files.
  Keep this backup read-only and record checksums. Rehearse restoration too.

Examples below use `/srv/migration/anvil-source` for a disposable, stopped
source copy, `/srv/solaris` for the new installation and a numeric seed of `42`.
Replace these with your paths and **actual seed**. Do not pre-create the native
or export destination: the converter requires a destination that does not exist.
Create their parent directories and the receipts directory instead.

### Paper layout and restrictions

Do not point import at the server installation root. It expects one world root
containing `level.dat` plus its dimension, entity and player stores. Vanilla
26.1.2 uses `dimensions/minecraft/{overworld,the_nether,the_end}` and
`players/{data,advancements,stats}`. Older layouts (`region`, `entities`, `DIM-1`,
`DIM1`, `playerdata`, `advancements`, `stats`) have explicit conversion mappings;
export uses the modern layout. Mixed old/new active locations are rejected.

Paper may keep the Nether/End in sibling world directories, and plugins may add
other worlds or files. There is **no Paper installation importer**, multiworld
merger or automatic dimension discovery across sibling directories. On a staging
copy, use the Paper version's documented return-to-vanilla procedure to assemble
one vanilla world, including each dimension's region, entities, POI and saved
data. Open and stop that copy with vanilla **26.1.2** first; verify all three
dimensions before importing. Do not overwrite duplicate saved-data paths or
arbitrarily choose between two `level.dat` files.

Only the three standard dimensions with the normal vanilla noise-generator
settings/biome presets are admitted. Superflat, amplified/custom generators and
custom dimensions are not a supported migration route. Do not rewrite their
worldgen metadata just to pass validation. Nonstandard files (including Paper
metadata and datapack directories) can cause a fail-closed import: retain them
outside the disposable source, understand their function, and decide whether
losing that function makes migration unacceptable. Never delete them from the
backup or the original server.

## 2. Import the licensed content cache

Content import is **not world import**. It extracts the registries and game data
that both conversion and serving require:

```sh
solaris-v2 content import --version 26.1.2 \
  --from /srv/minecraft/server-26.1.2.jar --cache /srv/solaris/content/26.1.2
# Alternative, not an additional required step:
solaris-v2 content import --version 26.1.2 --download \
  --cache /srv/solaris/content/26.1.2
solaris-v2 content verify --cache /srv/solaris/content/26.1.2
```

Import stages and validates the cache before publishing; a failed extraction
preserves the previous cache. Serving does not download missing content. Set the
same absolute `[data].vanilla_data_dir` in the conversion and serving config;
`--cache` applies only to its offline content command. See
[content selection](OPERATING.md#vanilla-content-cache).

## 3. Translate configuration explicitly

Solaris does **not** read `server.properties`, `bukkit.yml`, `spigot.yml`,
`paper-global.yml` or per-world Paper configuration. Copying those files has no
effect. Unknown TOML keys warn and are ignored; a successful start does not prove
that an invented mapping worked. Start with [example.toml](../example.toml),
not a mechanical hyphen-to-underscore rewrite.

A minimal, isolated rehearsal configuration at `/srv/solaris/server.toml`:

```toml
[server]
name = "Migration rehearsal"
motd = "Private migration test"
max_players = 20
view_distance = 8
simulation_distance = 6
accepts_transfers = false
pause_when_empty_seconds = 60

[network]
bind_address = "127.0.0.1"
port = 25566

[auth]
online_mode = true

[data]
world_dir = "/srv/solaris/native-world"
seed = 42 # MUST equal the saved world's actual numeric seed
vanilla_data_dir = "/srv/solaris/content/26.1.2"
```

Preserve the source authentication/UUID scheme. Do not switch an online server
to offline mode to make login easier: player inventories, ops, whitelist and
bans are UUID-keyed. For Velocity modern forwarding, configure `[proxy]` and
firewall the backend as described in [OPERATING.md](OPERATING.md); an offline
backend with no trusted forwarding is not equivalent. Relative paths resolve
against the **process working directory**, not the TOML directory. Access-list
files are the exception: they live beside the selected TOML.

### `server.properties` mapping

Each row describes the **property**, not a blanket verdict on the corresponding
game mechanic:

- **supported**: translate to the stated Solaris TOML key.
- **different**: configure another surface or account for a semantic difference;
  the property itself is not read.
- **ignored**: no equivalent operator property; do not paste it into TOML.

The inventory covers the properties emitted by vanilla 26.1.2. The final rows
also call out common older/Paper-era properties.

| Vanilla key | Status | Solaris mapping / migration action |
|---|---|---|
| `accepts-transfers` | supported | `[server].accepts_transfers` |
| `allow-flight` | different | No direct flag. `[anti_cheat].max_airtime_ticks = 0` disables the native airtime check, not all movement validation; test your flight use case. |
| `broadcast-console-to-ops` | ignored | No matching config switch. |
| `broadcast-rcon-to-ops` | ignored | No RCON configuration surface. |
| `bug-report-link` | ignored | No matching config switch. |
| `difficulty` | different | Imported saved difficulty/native `difficulty` command; no TOML difficulty property. |
| `enable-code-of-conduct` | ignored | No equivalent deployment setting. |
| `enable-jmx-monitoring` | ignored | No JVM/JMX; use Solaris dashboard/metrics. |
| `enable-query` | ignored | No legacy query toggle. |
| `enable-rcon` | ignored | No RCON listener; console and authenticated MSMP are separate interfaces. |
| `enable-status` | ignored | No status-enable toggle in TOML. |
| `enforce-secure-profile` | ignored | No matching policy knob; online authentication is not proof of equivalent signed-profile/chat enforcement. |
| `enforce-whitelist` | different | Login whitelist uses `whitelist.enabled`; do not assume vanilla's online-player recheck behavior. |
| `entity-broadcast-range-percentage` | ignored | No percentage knob. |
| `force-gamemode` | ignored | No forced-login gamemode property. |
| `function-permission-level` | ignored | Datapack function execution is unsupported. |
| `gamemode` | different | Saved player gamemode/native `gamemode` command; no default-gamemode TOML key. Check first joins separately. |
| `generate-structures` | ignored | No matching generation toggle; saved terrain is imported, future terrain uses Solaris generation. |
| `generator-settings` | different | Import validates saved normal-world descriptors; arbitrary settings/custom generators are rejected. |
| `hardcore` | ignored | No hardcore deployment setting; retained metadata is not proof of hardcore death/login semantics. |
| `hide-online-players` | ignored | No equivalent status-list privacy setting. |
| `initial-disabled-packs` | ignored | Datapack selection/execution is not migrated. |
| `initial-enabled-packs` | ignored | Licensed vanilla content cache is not a datapack loader. |
| `level-name` | different | `[data].world_dir` is the **new native** directory, not the Anvil source; `[server].name` is a display name. |
| `level-seed` | different | `[data].seed` is a signed integer matching the saved world; resolve blank/text seeds on the old server. |
| `level-type` | different | Only normal vanilla noise descriptors admitted; no flat/amplified/custom preset setting. |
| `log-ips` | ignored | No matching logging/privacy toggle. Review log handling separately. |
| `management-server-allowed-origins` | ignored | No equivalent allowed-origins setting. |
| `management-server-enabled` | different | `[msmp].enabled`; review Solaris's method/auth surface before enabling. |
| `management-server-host` | different | `[msmp].bind_address`; remote bind additionally needs `allow_remote`. |
| `management-server-port` | different | `[msmp].port` (default 25585). |
| `management-server-secret` | different | `[msmp].bearer_token`; set explicitly and protect it. |
| `management-server-tls-enabled` | ignored | No native TLS flag in `[msmp]`; do not expose a plaintext endpoint as though it inherited TLS. |
| `management-server-tls-keystore` | ignored | No Java keystore setting. |
| `management-server-tls-keystore-password` | ignored | No Java keystore setting. |
| `max-chained-neighbor-updates` | ignored | No matching config knob. |
| `max-players` | supported | `[server].max_players` |
| `max-tick-time` | ignored | No vanilla watchdog property; configure service supervision separately. |
| `max-world-size` | different | No corresponding cap setting; inspect imported border and native `worldborder` behavior. |
| `motd` | supported | `[server].motd`; use TOML string escaping, not Java `.properties` escaping. |
| `network-compression-threshold` | ignored | No corresponding operator key. |
| `online-mode` | supported | `[auth].online_mode`; default is **false**, so set it explicitly. |
| `op-permission-level` | different | Preserve per-user `level` in `ops.json`; no global config default mapping. |
| `pause-when-empty-seconds` | supported | `[server].pause_when_empty_seconds`; zero/negative disables pause. |
| `player-idle-timeout` | ignored | No matching idle-kick knob. |
| `prevent-proxy-connections` | ignored | Not mapped to Velocity trust; configure `[proxy]` explicitly. |
| `query.port` | ignored | No legacy query listener configuration. |
| `rate-limit` | different | `[anti_cheat].packet_rate_per_second` and `packet_burst` are per-Play-packet-id buckets, not the same algorithm. |
| `rcon.password` | ignored | Not an MSMP bearer token; no RCON listener. |
| `rcon.port` | ignored | No RCON listener. |
| `region-file-compression` | ignored | Native store + WAL while serving; no Anvil compression selector. |
| `require-resource-pack` | ignored | No equivalent vanilla resource-pack policy setting. |
| `resource-pack` | ignored | No automatic URL-to-Loader mapping. |
| `resource-pack-id` | ignored | No equivalent key. |
| `resource-pack-prompt` | ignored | No equivalent key. |
| `resource-pack-sha1` | ignored | No equivalent key. |
| `server-ip` | supported | `[network].bind_address`; choose an explicit address rather than copying an empty value. |
| `server-port` | supported | `[network].port` |
| `simulation-distance` | supported | `[server].simulation_distance` |
| `spawn-protection` | different | No vanilla radius knob; deploy and validate native/plugin protected zones if required. |
| `status-heartbeat-interval` | ignored | No matching deployment key. |
| `sync-chunk-writes` | different | Native WAL/checkpoints control durability; not a synchronous-Anvil switch. |
| `text-filtering-config` | ignored | No equivalent external filtering config. |
| `text-filtering-version` | ignored | No equivalent filter-version setting. |
| `use-native-transport` | ignored | Native Rust transport; no Netty selector. |
| `view-distance` | supported | `[server].view_distance` is an upper bound, capped by each client's request. |
| `white-list` | different | Copy `whitelist.json` and set `whitelist.enabled` beside TOML, or run `whitelist on`. |
| `allow-nether` (older/Paper) | ignored | No corresponding dimension-disable property. |
| `enable-command-block` (older/Paper) | ignored | Command-block execution is unsupported. |
| `pvp` (older/Paper) | ignored | No matching TOML switch; verify native gameplay/protection policy. |
| `spawn-animals`, `spawn-monsters`, `spawn-npcs` (older/Paper) | ignored | No same-named TOML toggles; review native game rules and compatibility. |

Paper-specific anti-xray, packet limiting and connection throttling must also be
translated intentionally: Solaris exposes `[anti_xray]`, `[anti_cheat]`,
`[network].connection_throttle_ms` and `max_connections_per_ip`, not Paper YAML.
See [supported configuration](OPERATING.md#supported-configuration) and
[MSMP](OPERATING.md#minecraft-server-management-protocol).

## 4. Move access lists, not identity assumptions

With Solaris stopped, copy these files from the old **server root** to
`/srv/solaris/`, beside `server.toml` (not into `native-world`):

| File | Handling |
|---|---|
| `ops.json` | Vanilla UUID/name/level/`bypassesPlayerLimit` rows are read directly. Check every permission level and bypass flag. |
| `whitelist.json` | Vanilla UUID/name array is read directly. Copying it does **not** enable enforcement. |
| `banned-players.json` | Vanilla player-ban rows, including reasons and expiration, are read directly. |
| `banned-ips.json` | Vanilla IP-ban rows, including expiration, are read directly; verify forwarded address policy behind a proxy. |
| `usercache.json` | No importer/reader in the edge admin path. Keep it in the backup for rollback, not as proof Solaris can resolve those names. |

If the old server enforced a whitelist, create a text file
`/srv/solaris/whitelist.enabled` containing `true` **before** first admission.
Alternatively enable it with console `whitelist on` before exposing the port.
Operators are not exempt from this whitelist. `whitelist reload` reloads a
hand-edited list. A missing list loads empty; malformed JSON fails startup.
Online-mode name lookups are not restored from `usercache.json`: preserve the
actual UUID rows, and test commands against a joined/already-known player rather
than assuming an arbitrary offline name resolves. Keep authentication mode and
proxy-forwarded UUIDs consistent with the source.

## 5. Convert the stopped world

```sh
solaris-v2 --config /srv/solaris/server.toml world import-anvil \
  --source /srv/migration/anvil-source \
  --destination /srv/solaris/native-world \
  --receipt /srv/migration/receipts/import.json
```

There is no `import-anvil --dry-run`. A migration rehearsal is a **real import
into a new disposable destination**. The receipt parent must exist, and the
receipt must be outside both source and destination. Conversion refuses live
worlds, checks seed/content/layout compatibility and preserves the source.
Do not work around a refusal by removing version tags, locks or native identity
metadata. Resolve the reported incompatibility on the staging copy, or stay on
the original server.

Read the receipt, not just the process exit: require `status: "PASS"`, then
review `counts`, `dimensions`, `normalizations`, `upgrades`, `known_gaps` and
`opaque_retention`. `PASS` may coexist with explicitly recorded preservation
losses. Duplicate entity UUIDs can drop later bodies with original NBT retained
as evidence; some empty unfinished chunks regenerate rather than survive.
The receipt's file inventory uses CRC32, not a cryptographic backup signature.

### What survives, and what does not

Storage retention and active gameplay support are different. Unsupported active
records can **reject** an import rather than silently disappear. Do not promise
lossless migration merely because an unknown field is kept in an opaque record.

| Domain | Migration behavior and limits |
|---|---|
| Blocks, biomes, chunk metadata | Stored chunks are converted to native baselines. Review unfinished/protochunk and scheduled-tick normalization counts. New terrain uses Solaris generation, not bit-identical vanilla; inspect explored-border seams. |
| Entities | Supported typed entity state and retained extra NBT are converted; native simulation can change them after start. Duplicate-UUID loss is receipted. Check villagers/trades, passengers, pets/owners, item frames, vehicles and named mobs individually. |
| Block entities | Supported containers and raw block-entity NBT are imported, but preservation is not universal. In the base build, typed container rewrites can drop extra metadata such as custom names/locks; verify these explicitly. Signs/banner/spawner/etc. NBT retention does not imply all behaviors execute. |
| Player data | UUID-keyed player NBT is imported; known inventory, position, XP, health, gamemode and related state have native owners, with extra fields retained where supported. Test a real returning player, ender chest, respawn and all dimensions. Missing-dimension fallbacks are explicitly loss-reported. |
| Advancements | JSON for admitted players is attached to native player state and emitted again on export; orphan documents are retained as extras. Custom criteria execution is not implied. |
| Statistics | JSON is parsed into native statistics and re-emitted. Unsupported data fails rather than being blindly copied; stats-only UUIDs can create native player records. |
| Maps | Saved map data is retained/converted through the saved-data path. Check item map IDs and rendered pixels; byte retention alone does not prove live exploration updates or decorations. |
| Scoreboards/teams | Native scoreboard persistence exists, but the inspected base rejects ordinary vanilla scores with `Locked: 1` (`unsupported locked scoreboard score`). A real 26.1.2 dummy-objective fixture hit this blocker; do not remove scores to claim a lossless migration. Older deferred scoreboard data can also conflict with a subsequently active native scoreboard and refuse export. Verify objectives, scores, team options and display slots on a build that admits your source. |
| Time, weather, game rules, spawn, border, dimension saved state | Imported through typed metadata, with remaining supported saved data retained. Verify values in each dimension, especially raids and the End fight. |
| POI and other opaque saved files | Admitted files are retained without executing their semantics/upgrades; unknown active `.dat` types can refuse import. Aggregate retained extras have a 16 MiB encoded-record budget. |
| Datapacks and custom dimensions | No datapack/function runtime. `datapacks/` is not an admitted opaque directory; a nonempty one can refuse conversion. Preserve it externally, and do not migrate a pack-dependent world unless the loss of its functionality is acceptable. |
| Resource packs | The specifically admitted world `resourcepacks/resources.zip` is retained; server.properties URLs/policies are not migrated or served automatically. Other arbitrary files can refuse import. |
| Bukkit/Paper plugin data | Not interpreted by world import. External SQL stores, plugin folders, claims, economy, permissions and custom items need a separately validated migration; keep originals for rollback. |
| Solaris-native plugin data on export | Vanilla cannot execute Solaris plugins or interpret their native state. Keep a native backup; Anvil export is not a plugin-state backup. |

Implementation boundaries: [world conversion](../crates/mc-core/src/world_conversion.rs),
[import](../crates/mc-core/src/world_conversion/import.rs),
[export](../crates/mc-core/src/world_conversion/export.rs) and
[admin lists](../crates/mc-edge/src/admin.rs).

## 6. Replace plugins deliberately

**Bukkit/Spigot/Paper JARs are not supported natively by the default Solaris
binary.** Do not copy a Paper `plugins/` directory into Solaris's package root.
The normal plugin surface is sandboxed Wasmtime components implementing
`solaris:plugin@0.8.0`, deployed as `plugin.toml` + `plugin.wasm` packages via
`[plugins].directory`; see [PLUGINS.md](PLUGINS.md). First-party packages live in
[solaris-default-plugins](https://github.com/kaiserproger/solaris-default-plugins).
Matching plugin names do not imply compatible configuration or databases.

Rehearse vanilla world conversion without plugins first. Inventory every old
plugin's function, persistence, commands and permissions; then deploy only
supported replacement packages, following their migration procedures. Registry
or worldgen-changing deployments can alter native world identity, so do not
assume that adding them after conversion is harmless. Permission/claim/economy
checks must pass before public login is enabled.

## 7. Start, verify, stop and prove rollback

```sh
solaris-v2 --config /srv/solaris/server.toml
```

Wait for the listening message and successful content/world/plugin admission.
On the private endpoint, join with an unmodified **26.1.2** client and complete
the checklist below. Stop using console `stop` and wait for process exit/final
checkpoint. Serving writes native `native/state.redb` and tick WAL, **not** the
old Anvil files. Keep the same config/content/deployment with the native world.

### Rollback option A: restore the untouched original

Stop Solaris and restore the entire original vanilla/Paper installation from the
pre-migration backup. Reapply the old port/proxy routing only after verifying it
starts. This is the most predictable rollback, but **discards all progress made
on Solaris**. Never mix the original player files with a newer world export.

### Rollback option B: export Solaris progress to Anvil

While Solaris is stopped:

```sh
solaris-v2 --config /srv/solaris/server.toml world export-anvil \
  --source /srv/solaris/native-world \
  --destination /srv/migration/anvil-export \
  --receipt /srv/migration/receipts/export.json
```

Require a `PASS` receipt and review gaps/normalizations again. Export recovers the
native checkpoint and WAL tail into a **new** modern 26.1.2 Anvil world; it is
not a downgrade to old Minecraft versions. Retain a native-world backup as well.

In a separate vanilla 26.1.2 test installation, put the exported directory at the
configured `level-name` path, retain your already accepted EULA, set a private
bind/port and the correct auth mode, and run:

```sh
java -Xms512M -Xmx2G -jar /srv/minecraft/server-26.1.2.jar nogui
```

Wait for vanilla's `Done` message, then check landmarks, containers, named mobs,
scoreboards and a real player. `save-all flush`, `stop`, and verify a second
start if this export will become the live world. Starting alone is not a
normalized-NBT comparison or inventory/gameplay proof.

Access lists live outside the world export: copy the **latest** Solaris
`ops.json`, `whitelist.json`, `banned-players.json` and `banned-ips.json` back
beside vanilla's properties, restore `white-list=true` as appropriate, and
retain the old usercache (a cache, not player state). Translate any changed
configuration back explicitly. For Paper, use that version's supported vanilla
layout intake and restore the matching plugin/database backups. Anvil export
cannot recreate Bukkit state or Solaris-only plugin effects that have no vanilla
representation. Validate this boundary before accepting player progress.

## Verification checklist before cutover

- [ ] Whole old installation backed up; restoration tested; source stopped and
  checksummed; exact binary/content version and numeric seed recorded.
- [ ] Every non-default property/plugin has an explicit supported replacement,
  accepted difference, or migration-blocking decision.
- [ ] Content verify passes; import destination is new; receipt is `PASS` and all
  `known_gaps`, drops, normalizations and opaque retention are reviewed.
- [ ] Overworld, Nether and End landmarks, spawn, portals, borders, weather,
  difficulty and game rules are correct; new-terrain seams are acceptable.
- [ ] Named/locked containers retain names, locks and exact items; signs, banners,
  furnaces, hoppers, redstone, maps and important block entities are checked.
- [ ] Pets, villagers/trades, passengers, vehicles and named entities survive.
- [ ] Returning players retain UUID, inventory, ender chest, XP, health, position,
  dimension, respawn, gamemode, advancements and statistics; a new join is tested.
- [ ] Scores/team options/display slots and any map IDs/rendering are checked.
- [ ] Operator and non-operator permissions, whitelist denial, active/expired
  bans and proxy identity/address behavior are tested before opening the firewall.
- [ ] Replacement plugins load and protect claims/economy/permissions correctly;
  no unsupported datapack/plugin behavior is being assumed.
- [ ] Clean Solaris stop/restart succeeds without changed seed/content identity.
- [ ] Export receipt passes; vanilla 26.1.2 opens the export and the same landmarks,
  containers and player state are verified; rollback losses are understood.
- [ ] Only then stop the rehearsal, schedule a final source freeze/import, and
  switch the public port/proxy. Keep the original backup and both receipts.

A headless fixture exercises only its named records; it is not proof of player,
Paper-plugin or full-world compatibility. See the local migration acceptance
report for the exact binary fingerprint, commands and failures from this change.
