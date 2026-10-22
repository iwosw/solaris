# ADR 0015 - Per-session protocol translation

**Date:** 2026-10-21  
**Status:** Proposed; 26.3 implementation deferred after the measured compatibility
surface exceeded a safe first slice.

## Context

`mc-core` is authoritative and version-independent; `mc-edge` already owns each
connection's state, Configuration exchange, packet decoding and presentation
planning. Solaris currently publishes only the 26.1.2 profile (protocol 775).
A newer vanilla client must not be silently dropped: a Status ping must continue
to advertise the supported release, while a Login attempt receives vanilla's
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
`mc_protocol::PROTOCOL_VERSION` and sends 775 in its handshake. It therefore
cannot walk a 777 server's Login/Configuration/Play path without first gaining
a version-specific codec profile; it is not valid evidence for 26.3 as-is.
The `javap` evidence above is the measured structural oracle. A future 777
profile must add a raw probe capture against a licensed 26.3 server before it
is accepted.

## Decision

Do not implement a 26.3 translation on this slice. The delta changes Login,
Configuration, registry data and core Play traffic at once, and the registry
resource change alone rules out a correct packet-ID remap. A real 26.3 client
join/play claim is consequently not made.

When a version is implemented, the boundary is a **per-session translation
layer in `mc-edge`**, analogous to ViaVersion:

1. The handshake selects an immutable `ProtocolProfile` from the client's
   declared version. Unsupported Login handshakes receive the vanilla
   incompatible disconnect; Status always emits Solaris' supported
   name/protocol.
2. `mc-edge` stores the selected profile in connection state. `mc-core`, its
   player inputs, player changes, persistence, plugins and world authority do
   not receive a protocol-version branch.
3. Profile-specific decoders map a peer packet ID/layout into the existing
   canonical edge input before the input is admitted to `mc-core`. Unknown or
   lossy packets fail the connection clearly before mutation.
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

The first implementation slice is a complete profile inventory plus typed
Login/Configuration codecs and registry mappings, not a partial mode that
advertises 777 then fails during join. Only after those prove equivalent may
core Play packet translations be introduced in focused vertical slices.

## Consequences

The current supported server profile remains 26.1.2 / 775. The direct
operator-visible improvement in this slice is a vanilla localized Login
refusal instead of a silent disconnect. The proposed boundary contains future
version conditionals at `mc-edge`; it does not change core authority or create
a second world/data authority.
