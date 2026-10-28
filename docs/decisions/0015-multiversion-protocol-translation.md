# ADR 0015 - Per-session protocol translation

**Date:** 2026-10-21 (implemented 2026-10-28)  
**Status:** Accepted. The server's canonical protocol is 26.3 (777); 26.1.2
(775) clients are served through the per-session translator below.

## Context

`mc-core` is authoritative and version-independent; `mc-edge` already owns each
connection's state, Configuration exchange, packet decoding and presentation
planning. At the initial investigation (2026-10-21), Solaris published only
26.1.2 (775). The 26.3 cutover now makes 777 canonical while retaining 775
sessions at the edge. Other Login versions receive vanilla's
`multiplayer.disconnect.incompatible` reason.

A local licensed 26.3 client was downloaded from Mojang's 26.3 manifest on
2026-10-21. Its SHA-1 is `e877b6a07acd633fb3bb475002175cec036e7b87`.
It and all derived material remain under ignored `.analysis/` paths, per ADR
0001 and ADR 0002.

## Measured 26.1.2 -> 26.3 delta

`javap` was run through `tools/dump-vanilla-protocol.sh` for both local client
jars. The raw registrations and record-field output are retained at:

- `.analysis/vanilla-26.3/protocol-26.1.2-javap.txt`
- `.analysis/vanilla-26.3/protocol-26.3-javap.txt`
- `.analysis/vanilla-26.3/protocol-26.1.2-to-26.3.diff`

`net.minecraft.SharedConstants` in the 26.3 client reports release network
protocol **777** and world version **5023**, versus Solaris' protocol 775 and
world version 4790. The registration/record diff is not an ID-only shift:

- The Configuration catalog grows from 30 to 31 entries: one clientbound
  `POST_EFFECTS` registration. The Play catalog changes from 210 to 213
  entries: five names added and two removed, shifting later IDs in their
  respective directions.
- Play clientbound adds `ADD_TRANSIENT_BLOCK`, `POST_EFFECTS`, and
  `SWING_ANIMATION`; Play serverbound adds `PUNCH` and replaces the separate
  spectate and swing packets with `SPECTATOR_ACTION`.
- The record inventory adds six entries (including those new packets plus
  `common/ClientboundPostEffectsPacket` and `game/MovementPacket`), removes
  two, and changes the declared fields of 15 existing packets. The changed
  packets include Login's `ClientboundLoginFinishedPacket` (a session UUID),
  entity position sync, explosion, particles, Login/respawn, vehicle movement,
  sign editing, chat signatures, advancements, teleport acknowledgement,
  chat, and item-on-block use. These changes affect both the join path and
  ordinary movement/action traffic.

The client resource registry inventory grew from 8,775 to 10,447 paths. Its
set-level diff is 1,913 added and 241 removed paths (2,154 changed names at a
minimum; same-name content edits are not included in that lower bound). Added
entries include 528 recipes, 432 worldgen resources, 351 structures, 249
advancements, 138 tags, and 123 loot tables; 227 worldgen resources, 12 tags,
and two villager trades disappeared. The exact lists are retained at:

- `.analysis/vanilla-26.3/registry-resources-26.1.2.txt`
- `.analysis/vanilla-26.3/registry-resources-26.3.txt`
- `.analysis/vanilla-26.3/registry-resources-26.1.2-to-26.3.diff`

The existing `wire-probe` is deliberately compiled against
`mc_protocol::PROTOCOL_VERSION`; it probes the canonical server, not the
legacy translator. A separate raw 775 TCP client in `legacy/join_tests.rs`
exercises the legacy Login/Configuration/Play path. The `javap` evidence
above remains the structural oracle; raw TCP tests are not graphical-client
acceptance.

## Decision

Use a **per-session translation layer in `mc-edge`**, analogous to ViaVersion.
An ID-only remap is insufficient: changed layouts and every registry-bearing
payload require the selected session's ids.

1. The handshake selects an immutable `legacy::Profile` for protocol 775.
   Unsupported Login handshakes receive the vanilla incompatible disconnect;
   Status advertises the selected supported name/protocol.
2. `mc-edge` stores the selected profile in connection state. `mc-core`, its
   player inputs, player changes, persistence, plugins and world authority do
   not receive a protocol-version branch.
3. Profile-specific decoders map a peer packet ID/layout into the existing
   canonical edge input before the input is admitted to `mc-core`. Unknown or
   untranslatable packets fail the connection clearly before mutation.
4. The existing canonical `PlayerChange`/presentation plan is converted at the
   edge into the selected profile's clientbound ID/layout. Translation happens
   before framing/compression and never in the tick or fanout authority.
5. Each profile owns its Configuration registry/tag/known-pack snapshot and
   explicit canonical-to-peer registry mappings. It may not reuse a 26.1.2
   numeric registry ID for 26.3 merely because an identifier happens to match.
6. A new profile is accepted only with local `javap` registration/layout
   capture, raw wire-probe Login -> Configuration -> Play capture, registry
   mapping tests, and a graphical client gate covering join, movement, chat,
   block interaction and inventory.


## Implementation (2026-10-28): 26.1.2 clients on the 26.3 server

The bump inverted the original plan: the canonical server protocol is 26.3
(777) and the translator serves 26.1.2 (775), the client base already in the
field. It lives in `crates/mc-edge/src/legacy/` and follows points 1-5 above:

- **Selection.** `handshake::read_handshake` accepts protocol 775 when the
  26.1.2 profile is loaded and installs a `legacy::Session` on the
  `Connection`. Status pings from 775 clients advertise `26.1.2`/775, so the
  server list shows the server as compatible. Without the profile, 775 logins
  keep the vanilla incompatible-version refusal.
- **Data.** The profile is loaded at startup from a second content cache: the
  26.1.2 cache beside the canonical one (`.../content/26.1.2` next to
  `.../content/26.3`, or `data/vanilla-26.1.2` next to `data/vanilla`;
  `SOLARIS_LEGACY_CONTENT_CACHE` overrides). Only its datagen reports
  (`packets.json`, `blocks.json`, `registries.json`), synced registry names,
  captured `reports/registry_network_nbt` and tags are used. A missing or
  unusable cache logs the reason and disables 775 logins, not 777 sessions.
  Upgrades retain the deployment's existing 26.1.2 cache; rc.1 `content import`
  remains 26.3-only. Native 26.3 singleplayer does not need the old cache.
  See [operator setup](../OPERATING.md#retaining-2612-clients-after-upgrading-to-263).
- **Hook.** The translator sits at the one encode/decode boundary in
  `connection.rs`: `Connection::send_packet`/`read_frame_*` before Play and
  `PlayReader::read_frame`/`PlayWriter::write_bytes` in Play. Pre-framed,
  possibly compressed fan-out batches are split, translated and re-framed per
  legacy session; canonical sessions take the old path unchanged. `mc-core`
  and the planner never see a protocol version.
- **Tables.** Packet ids, block states, items, entity types, block-entity
  types, sounds, particles, data-component types, attributes and the synced
  biome/damage-type registries are paired **by name** from both caches'
  reports, never by numeric id. Names 26.1.2 lacks are polyfilled by a small
  table in `legacy/tables.rs`: new stone families map to granite/end stone
  and sandstone variants, poplar wood to birch, coloured concrete/wool slabs
  and stairs to a similarly coloured existing slab/stairs (shape preserved),
  cushions to carpets, explorer maps to `map`, the sulfur cube to a slime,
  the cushion entity to an invisible `interaction`, `dappled_forest` to
  `forest`, `sulfur_caves` to `dripstone_caves`. Block states keep every
  property the substitute shares. Entities whose metadata layout differs
  (26.3 slime-like mobs extend `AgeableMob`: size moves 18 -> 16) are tracked
  from `add_entity` and their `set_entity_data` indices rewritten.
- **Configuration.** A 775 session announces `minecraft:core 26.1.2` and the
  26.1.2 registry entry names and tags, so the client uses its built-in
  definitions; 26.3-only `post_effects` is dropped.
- **Layouts.** Translated per the measured delta: `login_finished`
  (session UUID), `login`/`respawn` (spawn-info game modes, `onlineMode`),
  chunk sections (block/biome palettes; global palettes repacked from 16 to
  15 bits), block updates/events/entities, entity movement
  (`move_entity_*` step lists, `entity_position_sync` position path),
  `animate`/`swing_animation`, particles and explosions, sounds (unknown
  sounds inline by name), attributes, damage events, teams, item stacks in
  slots/equipment/metadata; serverbound `accept_teleportation` (pose filled
  from the tracked teleport), `player_action` ordinals, `sign_update`, item
  ids in creative slots and hashed container clicks. `swing` becomes `punch`,
  and `spectate_entity` becomes `spectator_action`. Commands, recipes, recipe
  book entries, advancements, merchant offers and statistics retain their
  presentations with registry ids remapped; advancement coordinates move back
  inside the legacy display record. Recipe tag slot displays remove the 26.3
  named item-HolderSet marker before writing the 26.1.2 bare tag identifier.
  The local 26.3 `javap -c -p SlotDisplay$TagSlotDisplay` static initializer
  selects `ByteBufCodecs.holderSet(Registries.ITEM)`; `ByteBufCodecs$34`
  writes VarInt zero before a named set's identifier. Omitting that marker
  makes the client consume identifier bytes as item registry ids during join.
  Nested item templates retain their item-first layout (not optional slot
  layout); pot decorations convert the new four optional templates into the
  legacy four-item list.
- **Failure policy.** Packets with malformed or untranslatable payloads fail
  the connection with context rather than silently discarding an inventory or
  world update. Explicitly new-only presentation packets (`post_effects`,
  `add_transient_block`) and unsupported new particles are omitted; registry
  entries with no representation (new attributes/statistics/block entities)
  are filtered at entry level. Item walkers cover every component Solaris
  admits for wire emission; arbitrary components beyond native admission are
  not a compatibility claim.
- **Reconfiguration limit.** The edge does not emit a Play-to-Configuration
  transition today. Only the initial Login → Configuration → Play path is
  supported here: after the connection splits, the writer cannot observe the
  reader's finish-configuration acknowledgement. Any future runtime
  reconfiguration feature must coordinate both translator halves before sending
  new Play packets.

Agent-run evidence (2026-10-28): 28 focused unit tests passed. The end-to-end
test `legacy::join_tests::legacy_client_joins_and_sees_polyfilled_chunk` passed:
a 775 handshake reaches Play on the 777 server, both known-pack and full legacy
RegistryData exchanges carry legacy tags, and a translated block update and
chunk replace 26.3-only cinnabar with the independently loaded 26.1.2 granite id.

The actual Minecraft 26.1.2 graphical client (NeoForge 26.1.2.76 plus the
automation agent, under Xvfb) also passed two existing harness profiles against
the release server, both on seed 81:

- `survival`: join, ordinary mining, drop pickup, hotbar selection, placement
  and item consumption.
- `inventory`: cursor round-trip conserves two apples, crafting consumes one
  oak log and creates four planks, the grid/cursor end empty, and selection
  preserves inventory. This is the bounded adjacent inventory/duplication pass.

Receipts are under `.analysis/validation/20261028T065419-survival-qu1chur_/`
and `.analysis/validation/20261028T065648-inventory-lyaogt0n/` in the validation
checkout, including `result.json`, `scenario.json`, `server.log` and `client.log`.
Both used release-server SHA-256
`827e4169b78242604bbeba36a81c5fb5e30e61ad8c8122b73afdbe1f9dbd4fed`.
These are bounded agent-run passes, not certification of every packet or a
canonical 777 graphical-client gate. Both clients logged nonfatal trial-spawner
NBT errors (`Registry does not exist: minecraft:trial_spawner`); trial-spawner
presentation is an observed remaining gap, not covered by these passing scenarios.

Reproduce from the checkout root (wrap heavy commands with the shared-machine
memory runner as required by `AGENTS.md`):

```sh
cargo test -p mc-edge --lib legacy::
cargo test -p mc-edge --lib legacy::join_tests::legacy_client_joins_and_sees_polyfilled_chunk -- --ignored --exact
cargo clippy -p mc-edge --lib --tests -- -D warnings
cargo build --release -p mc-edge --bin solaris-v2
SOLARIS_SERVER_BINARY="$PWD/target/release/solaris-v2" python3 -m tools.harness run survival --client java --timeout-seconds 120
SOLARIS_SERVER_BINARY="$PWD/target/release/solaris-v2" python3 -m tools.harness run inventory --client java --timeout-seconds 120
```

The ignored TCP gate requires the canonical 26.3 cache at `data/vanilla` and
the retained 26.1.2 cache selected as above; absent caches are an error, not
a skipped/pass result. Ordinary translation unit tests use synthetic reports
and do not require Mojang data.
Set `SOLARIS_SERVER_BINARY` to the actual Cargo target path if
`CARGO_TARGET_DIR` is overridden, and `SOLARIS_LOADER_ROOT` to the checkout
containing the 26.1.2 graphical automation agent.
