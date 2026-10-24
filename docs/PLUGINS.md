# Plugins

Solaris production plugins are Wasmtime components implementing the versioned
WIT contract `solaris:plugin@0.8.0`. The host crate is
`crates/mc-plugin-host`; the WIT source is `crates/mc-script/wit`; Rust guest
source and the SDK live in the separate `sdk/rust/` workspace. The server
loads a compiled component artifact. It neither compiles guest source nor
requires a sibling checkout to start a production deployment.

A plugin is an addon, not a server mod. It receives typed, bounded values over
the WIT boundary; it never receives world, entity, region, session, socket,
lock, task, or scheduler handles. Native owners retain authority for every
world mutation, storage operation, and player session.

## Operator deployment

The configured plugin root contains one directory per package:

```text
plugins/
`-- my-plugin/
    |-- plugin.toml
    |-- plugin.wasm
    `-- config.toml       # optional
```

`plugin.toml` declares the package identity, API version, subscriptions,
command roots, requested capabilities, and optional client, world-generation,
or precommit declarations. `plugin.wasm` is a WebAssembly **component** for
`solaris:plugin@0.8.0`, not an unencoded core Wasm module. `config.toml` is
optional opaque package configuration: the host passes its text to the guest,
which defines and validates its own settings.
The host opens configuration relative to the package directory, refuses symlinks
and non-regular files (including FIFOs), and bounds the bytes actually read by
`PluginLimits::text_bytes` (8192 by default). Only a missing file means empty
configuration; unsafe, unreadable, oversized or non-UTF-8 files refuse loading.

### Native server

`solaris-v2` uses the production component host with a directory-only deployment:

```toml
[plugins]
directory = "plugins"
```

```sh
solaris-v2 --config server.toml
```

Every entry must be a valid package directory; malformed manifests or WASM,
duplicate identities/command roots, failed startup, and unsupported startup
declarations refuse startup rather than skip a package. Selecting this directory
authorizes its manifest-declared ordinary capabilities. The retired deployment
strict/expected/grants/reload and explicit hook-roster settings are not supported;
unknown configuration keys warn and are ignored. Every manifest-declared
`before-build` hook registers automatically in deterministic package-id order
with fail-closed (`deny`) policy. Unsupported hooks, including `before-damage`,
fail startup before component compilation or native world admission. There is
no server `--check`: startup performs real validation before serving.

The v2 edge obtains a native read-only build plan, asks the production component
host outside the tick, then submits a single-use frozen authorization. One
queue-inclusive 100 ms deadline covers preview and the host handler chain,
ending when the host answers. A timely answer may wait for the next native
merge without expiring; consumption still rechecks roster/source validity,
single-use, native state and plugin-KV versions. Click-time orientation and ray
origin stay frozen; current reach, occupancy and support still validate.
A native-input FIFO fence prevents previews from overtaking
earlier movement or hotbar changes. An idle native owner can retire metadata
fences and answer ready read-only previews on an explicit wake without creating
an extra tick or renewing the deadline.
Denial, timeout, unavailable/retired handlers and stale native or
plugin-KV guards repair client block predictions and inventory without creating
a world edit or block-WAL record; the tick never waits for a guest.

The covered native operations match the old build-hook call sites: player block
breaks and placements (including paired/construction cells), bone-meal growth,
successful composter fills, clicked/raycast bucket edits, and server-owned block
edits (`SetWorldBlock`, plugin/NPC physical structure portions, and NPC harvest,
tree-cut, mine and replant work). Ordinary interactions, tools such as hoes or
ignition, environmental simulation and damage are not build hooks; protected
zones (below) cover them natively instead.
Hook-free deployment keeps the direct native path.

With `[auth].require_plugin_login = true`, the native session gate denies
gameplay until an authentication plugin unlocks that session. `UnlockSession`
requires the explicit manifest capability `auth`; observing `player.joined`
alone grants no unlock authority. Only `solaris-auth` in the standard pack
declares `auth`. It does not register a `before-build` hook: the native login
gate is the single login-lock enforcement path.

Registered commands are advertised in the actual client Commands tree and
dispatch to the loaded component. A manifest-owned root takes precedence over
an otherwise available native root, including its manifest permission in the
tree, help and completions. Its immutable deployment reservation survives guest
retirement: unavailable admission returns an error, never native-command fallback.
Native aliases not claimed by a manifest retain their native behavior.
Plugin KV and native effects share v2's native checkpoint and tick WAL; there are
no separate plugin state files. The journal worker maintains the committed plugin
image and serializes it for checkpoints outside the tick. Operation callbacks and
command replies follow successful journal append completion, not fsync, and an
unflushed crash tail can consistently roll back an acknowledged effect.

Operation-id receipts remain durable for the world's lifetime. A recorded id
cannot execute again: retries return its original outcome and conflicting reuse
is refused, including after native checkpoint/WAL recovery. There is no age/count
eviction or id-reuse horizon. Receipt memory and checkpoint size can therefore
grow with cumulative operation ids; serialization is off the tick, but retention
remains intentionally unbounded rather than weakening exactly-once effects.

Inventory reservations retain at most 4096 active or structure-bound references,
plus the latest 256 unbound terminal references. Committed structure-bound
references remain pinned while their structure needs them; capacity exhaustion
refuses new admission with `Busy`. Unbound terminal references leave a bounded
FIFO and cease to resolve after eviction. Custody checks use native source
indexes, and mutations journal individual reservation records rather than a
whole historical ledger.

Observations have a separate 256-event queue per registered plugin. A full queue
drops the newest observation and increments
`ScriptBoundary::observation_events_dropped(plugin_id)`; round-robin dispatch
prevents one observer from owning the whole observation backlog. Durable native
completion callbacks and command replies are not shed: exhaustion of their
retained capacity fails closed. Timers advance only from acknowledged native
ticks. Optional Loader bundles use the existing configuration
manifest/artifact/ACK protocol; sound requires that session's actual Loader
readiness and the bundle's sound permission.


## Protected zones

`upsert-protected-zone` (capability `zones`) gives a plugin-owned box a native
allow-list of up to 32 player UUIDs plus a `public-use` flag; `upsert-zone` is the
event-only variant. The native owners enforce the list inside the box on every
tick path, with no guest call and no deadline: block break/place, buckets,
bone meal and every block use (so opening chests, barrels, furnaces and other
containers, and ignition by flint and steel or fire charges, are refused),
interaction with entities (item frames, armor stands, animals, villagers, boats,
minecarts, horses), and damage from a player's melee, sweep, mace or projectile to
anything but a hostile mob (PvP included). Operators are always admitted.
Explosion blasts spare every protected block and fire (spread from fire blocks or
lava) never enters one, whoever caused them. A zone with `public-use` additionally
admits everyone to doors, trapdoors, fence gates, buttons and levers, and only
those. Refusals repair client prediction without a world edit. An unlisted
hopper or piston still moves items and blocks across a boundary; claims guard
players and environmental fire and blasts, not redstone machinery. A zone
is keyed by its owning plugin, replaced atomically by re-sending the command, and
persists in the native checkpoint and WAL.

## Death graves

The manifest capability `death_graves` is a deployment surface rather than a
command grant: it changes what the native death path does for every player, so an
operator authorizes it by deploying the package that declares it. With the
capability deployed and `keepInventory` off, a player death moves the exact
dropped stacks into a `minecraft:barrel` placed at the death spot in the same
tick, journaled with the rest of the world; the packs a barrel's 27 slots cannot
hold are dropped beside it, so nothing is lost when a full inventory does not fit.

The placement tries the death cell, the cell above it, then a ring of radius 1–2
at the death level, one above and one below. A cell inside another package's
protected zone is skipped, so a grave never appears in a claim. Every candidate
must also stand on full-block collision ground that is not already a hopper: a
slab or a rail is not enough, because the remaining space in that cell is what a
hopper minecart needs. The full cell is what the zone can keep full, so no hopper
block, rail or minecart can be placed or rolled under the grave later, and an
existing hopper directly beneath it already pulls. A death in mid-air or over a
fluid therefore moves the grave to the nearest grounded ring cell, and when no
free grounded cell exists the drops stay ordinary items. Vanishing-curse stacks are destroyed by the
death before custody, exactly as in vanilla, and experience stays with the
player, because Solaris spawns no death XP orbs. The `keepInventory` gamerule wins
over the declaration: a retained inventory never reaches a grave.

The server then publishes `player.grave_created` to the declaring package only
(the event is targeted, like `inventory.menu.clicked`), carrying the owner, the
dimension, the grave cell and how many stacks went in and spilled. Product policy
- protecting the grave for its owner, announcing the coordinates and making it
public after a timer - belongs to that package and uses the zone owner above. One
deployment may declare the capability once; a second declaring package refuses
startup.

## Package manifest

A minimal server-only component package can declare no subscriptions,
capabilities, or player commands:

```toml
id = "hello"
name = "Hello"
version = "0.1.0"
api = "0.8.0"
```

The optional `entry` names a component file inside the package and defaults to
`plugin.wasm`. `events`, `capabilities`, `required_features`,
`player_commands`, `command_permissions`, `permission_provider`, `hooks`,
`[client]`, and `[worldgen]` are validated package declarations. Unknown fields
and invalid or duplicate values refuse the package before guest code is
compiled.

`required_features` describes authored data core must load. It does not grant
access. Feature capabilities must also appear in `required_features`. Packages
declare ordinary capabilities in their manifests; selecting the deployment
directory is the operator authorization decision.

Packages without `[client]` bundles are `server_only` and support ordinary
vanilla clients. Any package that declares client bundles makes the relevant
deployment `server_and_client`: connecting players need a compatible Solaris
Loader and must approve the bundle's requested client permissions. Bundle paths,
hashes, sizes, content kinds, supported loaders, and permissions are validated
before a player is served content. There is no server-only substitute for a
package that requires client content. See [`SOLARIS_LOADER.md`](SOLARIS_LOADER.md)
for player installation.

A bundle may set `required = false` (default `true`) in its `[[client.bundles]]`
table for content that enriches rather than enables the package, such as a custom
item texture. When every bundle of a deployment is optional, Configuration sends
a `solaris:loader/probe` payload (empty) before known-pack negotiation. A Loader
client answers `solaris:loader/hello` (empty) while handling it, so the hello
precedes its known-pack reply; the server then runs the normal manifest/artifact/
ACK negotiation. A client that does not answer (a vanilla client, or a Loader
build that predates the probe) joins without any Loader content, and its item
stacks omit non-`minecraft:` `item_model` components so plugin items show as
their carrier item with their custom name rather than the missing-model cube.
One required bundle anywhere in the deployment keeps the unchanged behavior:
every client negotiates and a client without the Loader is refused.

## Build, encode, and deploy a Rust guest

The SDK is an API dependency, not a home for product plugins. A guest's source,
manifest, configuration, and deployed artifact belong to that plugin's own
repository. Build its `wasm32-unknown-unknown` module, then encode that module
as a component before placing it in its package:

```sh
rustup target add wasm32-unknown-unknown
cargo build --manifest-path path/to/plugin/Cargo.toml \
  --target wasm32-unknown-unknown --release
cargo run --manifest-path crates/mc-plugin-host/Cargo.toml \
  --example plugin-component -- \
  path/to/plugin.wasm plugins/my-plugin/plugin.wasm
```

`plugin-component` validates while encoding; a module that lacks the component
types cannot become a deployable artifact. The plugin repository owns
`plugin.toml` and optional `config.toml`; the core repository neither ships nor
compiles first-party product plugins.

Authors implement `solaris_plugin_sdk::Plugin` (or bindings for the same WIT
world) and use the WIT and SDK definitions as the API reference. Do not copy
internal host types into a guest or rely on a host implementation detail.

WIT record layout changes require regenerating both host and guest bindings.
Rebuild every guest that uses `client-presentation` after the optional marker
position extension; an older component's record layout is not a compatible
fallback. Rebuild every guest against `solaris:plugin@0.8.0`.

## Lifecycle and configuration

A component has two startup phases in distinct stores:

1. `configure` receives the package configuration in a short-lived store with
   no runtime capability. It may return a normalized startup contribution that
   is validated and fingerprinted into the world contract.
2. `init` receives the same configuration and initialization context in the
   component's runtime store. It creates the instance's runtime-local state and
   may return its opening command batch.

State retained during `configure` cannot reach `init`; initialize runtime state
from the configuration again. The host performs discovery, manifest validation,
startup contribution validation, and deployment-surface validation before it
opens a world or binds the listener. A conflicting startup contribution,
world-generation declaration, client surface, or command ownership refuses
startup.

`shutdown` is best effort. The host reclaims its own resources whether or not
it runs, and a trap can skip it, so no durable correctness decision may depend
on cleanup. Plugin storage remains keyed by plugin identity in the native tick
WAL/checkpoint. Restart creates fresh component-local state; it is not a guest
instance migration. The current server does not support SIGHUP deployment reload.
Retain the same startup/world identity when restarting an existing world.

## Custom items, crafting recipes and block tools

`configure` may declare up to 16 plugin-owned `items` (a vanilla `carrier`
identity, stack size, optional durability), which need a Loader bundle with
`register_items` to present their own model/texture. Two further startup
fields extend them, both immutable, bounded and fingerprinted into the world
contract (omitted from it when unused):

- `crafting-recipes` (at most 32 strings, 64 KiB): vanilla-format
  `minecraft:crafting_shaped` / `crafting_shapeless` recipe JSON. The recipe id is
  `result.id`, which must be one of the plugin's declared items; every ingredient
  must be a registered item, an owned item or a known item tag.
- `block-tools` (at most 1024 strings, 1 MiB): rules for what a held plugin-owned
  item does to the block it is used on. A rule is strict JSON
  `{"item","block","when"?,"face","result","set"?,"wear"?,"give"?}`: the held
  `item` used on `block` (whose properties include `when`) at a clicked `face`
  (`up`, `down`, `side` = any horizontal face, or `any`) replaces it with
  `result`, whose default state takes the `set` properties. A `set` value is a
  literal, `"$face"` (the clicked face name) or `"@name"` (the source block's
  property `name`). `wear` (default 1) is durability charged and `give` one
  vanilla item granted to the player (dropped at their feet when the inventory is
  full). Creative players pay no wear and receive no grant; main hand only. The
  first rule of the item and block matching the face, `when` and a changing result
  wins. Startup resolves every rule against the block registry for every face it
  covers and refuses the deployment when a block, property or value does not
  exist.

The use is one native intent (`IntentKind::BlockTool`) planned beside hoe tilling in
`use_item_on`: the block replacement, the wear and the grant commit or are refused
together against the world and inventory they were planned on. Protection zones
and reach are checked by the same early use gate as block placement; plugin
before-build hooks do not cover it, like hoes. The `chisel` package in
`solaris-default-plugins` is the reference user.

## Custom native mobs

API **0.8.0** adds `mob-blueprints: option<list<string>>` to the configure
contribution. Each string is a strict `MobBlueprintDef` JSON object (see
[ADR 0016](decisions/0016-mob-capabilities.md)); a package may contribute up to
64 strings, each at most 16 KiB, and the complete registry allows 64 custom
types. Each id must use its defining plugin's namespace. Definitions are sorted
by id before registration and pinned into the world contract. Changing or
removing them refuses an existing world rather than reinterpreting stored mobs.

`sdk/rust/examples/sky-creeper` is a complete Rust guest: it contributes
`sky-creeper:sky_creeper`, presented as a vanilla creeper, with flying movement,
a 20-tick non-griefing explosion and water/rain damage. Its `/sky-creeper`
command spawns one near the calling player through the normal native owner:

```toml
player_commands = ["sky-creeper"]
spawn_entity_types = ["sky-creeper:sky_creeper"]
```

The guest `spawn-entity` command carries a request id, live player session,
namespaced entity type and position in that player's dimension. The manifest
allow-list authorizes type names, not bypasses of session or capacity checks.
`entity-spawn-answered` reports success (`failure = none`) or a typed refusal.
Each defining plugin has a 1024-live-custom-mob limit across dimensions; splitters
reserve descendant capacity as described in ADR 0016. Custom types have no natural
spawn weight. Native checkpoints retain their custom identity; Anvil export uses
the vanilla presentation id and a `solaris:MobBlueprint` marker, not custom
gameplay on a vanilla server.

**Rebuild all guests for 0.8.0.** The Rust SDK workspace version is 0.8.0.
Regenerate WIT bindings, add `mob_blueprints: None` to existing full
`StartupContribution` literals, update manifest `api`, and rebuild/re-encode
every component. This also applies to `../solaris-default-plugins`; no 0.7.0
component or old record-layout shim is accepted.

## Events, commands, and capability boundaries

A manifest subscribes only to the named events it needs and claims only its own
player-command roots. The host delivers events to one component in order. Each
callback receives immutable, bounded records and returns a bounded command
batch; host admission validates every command before native work is scheduled.
An oversized, malformed, unauthorized, trapped, budget-exceeding, or rejected
callback does not expose a partial command batch. A command's eventual result
is delivered as a typed event when the owning native service completes it.

Capabilities gate privileged operations such as storage, player queries,
teleportation, zones, inventory, or world changes. They are not client
permissions and they do not make a request authoritative. Every native owner
still checks the live player session, ownership, revisions, input bounds,
capacity, and its domain invariants before committing work.

### Command permission nodes

A manifest may require one node per command root it already declares:

```toml
player_commands = ["home", "sethome"]
command_permissions = { home = "solaris.essentials.use", sethome = "solaris.essentials.use" }
```

Nodes become enforced when the deployment installs one permission provider
package:

```toml
permission_provider = true
```

The provider is the only instance handed the `publish-command-permissions` host
import. It replaces the whole grant table with `"<identity> <node>"` rows:
`identity` is a dashed or compact lower-case UUID, or `*` for the default row
that applies to a player the table has no explicit row for; a node of `*` grants
every node. Rows are rebuilt from the provider's own durable state, so the table
is not persisted by the host. A publication is bounded (8192 entries, 192 bytes
per entry, 128-byte nodes), all-or-nothing, and refused during a pre-commit hook
like every other import.

Command admission checks the table on the tick thread before an owned root
receives an event. A denied root answers `You do not have permission to use this
plugin command.` and never reaches the guest; operator-only roots keep requiring
operator level 2 first. A second `permission_provider` package, or a node for a
root the package does not declare, refuses the deployment. In a deployment that
has never installed a provider, node declarations are inert. Once managed,
provider retirement clears grants but keeps gated roots denied until an accepted
provider publishes again; a guest trap never returns the deployment to unmanaged
admission. The host's component-reload API stages candidate grants separately,
rejects manifest-only contract changes before guest execution, and installs the
staged snapshot only on acceptance. Rejected initialization and old-generation
shutdown cannot publish to the live table.

`client-presentation.view-marker.position` is optional, display-only geometry:
a resource-identifier dimension plus finite coordinates within the native
`types.position` bounds. The host validates those bounds and counts dimension
text against the same command staging budget as other model strings. Geometry
does not query world state, replace selection fields or their context token,
grant an action, or authorize a physical route or world mutation.

Use stable request and operation identifiers where the WIT operation requires
them. A successful submission is not confirmation of client rendering or a
future mutation; inspect the typed completion or refusal event. For durable
operations, reuse an operation id only for the same intent and payload.

Precommit hooks are a separate bounded decision path. Directory-only deployment
automatically registers manifest `before-build` declarations and rejects
unsupported kinds, including `before-damage`. Hooks receive immutable contexts
and answer decisions, not command batches. They cannot perform I/O, submit
asynchronous work or mutate the world. Missing, expired or retired handlers
follow the fail-closed policy described above.

## Diagnosing a deployment

Startup validates manifests, components, API/capability/event names, duplicate
identities and command roots, client artifacts and startup answers. There is
no separate deployment `--check`; `content verify` checks only the vanilla cache.
Do not confuse it with plugin or gameplay validation.

At runtime, act on traps, budget failures and admission refusals. A failed
component is not a successful degraded deployment: owned command roots remain
reserved and unavailable, never native-command fallbacks. Repair or explicitly
remove the package and restart with a valid deployment.

Ordinary callbacks have a 50,000,000-unit guest fuel allowance and a separate
1,200-epoch hang guard (approximately 30 seconds with the production 25 ms
ticker). Fuel bounds guest instructions, not host scheduling delay. The hang
guard includes time inside native imports and descheduling; it interrupts at
the next Wasm check, not inside blocking native code. In particular, native
Argon2id password work retains its fixed 19 MiB, two-pass, single-lane cost and
does not consume guest fuel. Precommit hooks still obey their short absolute
chain deadline rather than the ordinary callback allowance.

With required plugin login, the configured deployment must contain an
`auth`-capable package. Every such provider is required: retirement logs the
package identity, failure and `restart required` at ERROR once. Pending and new
unauthenticated sessions are disconnected with an authentication-unavailable
reason; already authenticated players keep playing and native ticks continue.
The login fence stays enabled and a trapped Store is never retried. Inspect
the preceding callback error to distinguish fuel exhaustion from the wall-clock
hang guard, repair the package or host capacity, then restart the server with
the same world and account data. Do not disable required login as a recovery
shortcut. Other retired components retain the isolated command-unavailable
behavior above.

## Bukkit compatibility (experimental)

`tools/bukkit/convert.py` is an **explicit offline converter**, not a JVM server
backend. It takes an ordinary Bukkit/Spigot plugin JAR, reads `plugin.yml` and
annotated handlers without executing plugin classes, compiles reachable Java
bytecode with TeaVM 0.10, and links it with a Rust SDK adapter into a normal WIT
component. Deploy only the resulting `plugin.toml` and `plugin.wasm` using the
directory layout above. The ordinary server build and runtime do not require
Java, Maven, TeaVM, Binaryen, or an additional Cargo feature. Nothing loads JARs
automatically; native plugins and core gameplay remain unchanged.

This is a small API compatibility slice, **not CraftBukkit, Spigot or Paper
compatibility in general**. There is no CraftServer/NMS implementation, embedded
JVM, Java agent, or second Wasmtime backend.

Supported surface:

- `JavaPlugin.onEnable/onDisable`, `getLogger().info`, `getServer()`,
  `Bukkit.getServer/getPluginManager`, and `PluginManager.registerEvents`.
- Manifest-declared lowercase commands, `getCommand(name).setExecutor`,
  `CommandExecutor.onCommand` (including the JavaPlugin override), command name,
  label and argument array. A false result emits the declared usage, if any.
- `PlayerJoinEvent.getPlayer`, `Player`/`CommandSender.getName`,
  `Player.getUniqueId`, and `sendMessage(String)`. Messages become ordinary
  session-addressed WIT commands; they do not write directly to a socket.
- Public instance `@EventHandler` methods for `PlayerJoinEvent` and
  `BlockBreakEvent`, ordered by `EventPriority`, with `ignoreCancelled`.
  Listener classes must be public and available in the input JAR. This is
  statically generated dispatch, not reflection-based event discovery.
- `BlockBreakEvent.getBlock/getPlayer/isCancelled/setCancelled`, immutable
  block coordinates and `getType`, and block-material enum constants generated
  from the deployment's vanilla `reports/blocks.json`.

The break mapping is deliberately narrower than Bukkit: a player-authored
**non-air to air** cell in a native `before-build` batch creates an event.
It also covers non-mining removals represented that way; it does not distinguish
the action cause. A cancellation rejects the **whole native batch**, including
paired cells. Placements and non-player actors do not create a break event.
There are no XP/drop controls or world mutation methods. The precommit snapshot
has UUID/session but no player name: asking for that name throws. Logging or
messaging inside the hook traps rather than leaking effects from a rejected
build. Native admission, frozen authorization and the native tick still decide
whether an allowed operation commits.

Use the exact same block report for conversion and deployment. The output
`conversion.json` records SHA-256 of the input JAR and report for the operator;
the runtime cannot authenticate that report against native state. Reconvert
after changing the vanilla cache. Unknown/custom state IDs fail closed, but a
different registry reusing an existing ID cannot be detected by this subset.

Unsupported: dependencies/soft dependencies, aliases, permission metadata,
configuration/resources, scheduler/threads, networking/filesystem, reflection,
dynamic class loading, other events, arbitrary Bukkit APIs and Paper/NMS calls.
Unsupported metadata/events are rejected explicitly; unavailable reachable
methods fail Java/TeaVM compilation or component linking. A successful
conversion does not certify unexercised plugin behavior. Fatal Java exceptions,
fuel/memory limits and invalid output use the existing host failure path.
The transport caps a string at 8192 bytes and a returned batch at 256 messages;
ordinary host/native validation may impose tighter bounds.

### Offline build and receipt

Required local tools: JDK **17**, Python 3 with PyYAML, Maven 3.9, Binaryen **123**
(`wasm-opt`, `wasm-merge`), Rust with `wasm32-unknown-unknown`, and the normal
Solaris dependencies. Maven resolves the pinned TeaVM dependencies in
`tools/bukkit/pom.xml`. The offline compiler toolchain is **not a sandbox**;
convert trusted input only (compiler extensions may execute build-time code).
The converter does not intentionally invoke the plugin lifecycle. Build the
adapter and existing component encoder from the **same Solaris checkout**:

```sh
cargo build --manifest-path tools/bukkit/adapter/Cargo.toml --target wasm32-unknown-unknown
cargo build -p mc-plugin-host --example plugin_component
python3 tools/bukkit/convert.py \
  --java-home /path/to/jdk17 --maven /path/to/mvn \
  --binaryen-bin /path/to/binaryen-version_123/bin \
  --jar /path/to/plugin.jar --blocks-report data/vanilla/reports/blocks.json \
  --adapter tools/bukkit/adapter/target/wasm32-unknown-unknown/debug/solaris_bukkit_adapter.wasm \
  --component-encoder target/debug/examples/plugin_component --output /tmp/converted-plugin
```

On shared agent machines, run Cargo through the repository's memory slot runner
and point `--adapter`/`--component-encoder` at that runner's target directory.
The converted manifest declares join subscriptions, command roots and the
build hook; no compiled-in plugin list is added.

`tools/bukkit/fixture` is an ordinary plugin source with logging, `/hello`, a
join greeting, and diamond-ore cancellation. `build_fixture.py --java-home ...
--spigot-api /path/to/upstream-spigot-api.jar --output /tmp/bukkit-receipt --
<converter arguments except --java-home, --jar, --output>` compiles it against
the **upstream API**, never the shim, then uses the same generic converter.
Run the opt-in in-process host test against its output:

```sh
SOLARIS_BUKKIT_PACKAGE=/tmp/bukkit-receipt/package \
SOLARIS_BUKKIT_BLOCKS=data/vanilla/reports/blocks.json \
  cargo test -p mc-plugin-host --test bukkit_component -- --ignored
python3 -m unittest discover -s tools/bukkit -p 'test_*.py'
```

The test exercises normal package validation, isolated configure, lifecycle,
UTF-8 join/command messages with exact session targets, native precommit
keep/cancel decisions and unknown-state retirement. It is not a real-client
or full third-party-plugin compatibility test. Upstream API JARs, Mojang
reports, generated Java/Wasm and downloaded tools remain local artifacts.
