# ADR 0018: opt-in in-game computer

Status: accepted design; draft implementation, real-client QA pending.

## Decision and alternatives

Keep world authority in the native core. A computer is an ordinary **iron block**
placed/broken/persisted through vanilla gameplay. On an explicitly enabled Fabric
client, right-click opens a text terminal. The edge validates the authenticated
operator, current dimension, loaded carrier and proximity against published native
state. No custom block registry, inventory alias or second world store is needed.
When disabled, iron blocks retain normal behavior.

A Wasmtime plugin is preferable for gameplay, but the current `solaris:plugin@0.7.0`
WIT has neither arbitrary Play payload/process spawning nor a block-use callback
(`world-events.wit` exposes entity interaction). Do not extend guest authority to
host process execution for this feature. Use a small off-by-default edge service
and a separate `solaris:computer` Play channel. Existing Loader wire 3 is unchanged.

Rejected: embed a Wayland compositor (Linux-only), stream the server's desktop
(privilege/privacy violation), or grant untrusted Loader bundles capture/input
permissions. The terminal is a line-oriented session, not a PTY emulator: output
is plain UTF-8; interactive curses apps need a different adapter.

## Security and lifetime

`[computer] enabled = false` by default; `local_only = true`. Every open/input
requires unlocked operator level 4. Local-only means the actual TCP peer is
loopback **and no forwarding proxy**. Remote opt-in additionally requires online
Minecraft authentication or authenticated Velocity forwarding. Offline username
spoofing is not remote operator authentication.

Default backend is a built-in echo, not a shell. `command = [absolute executable,
arguments...]` is an operator-selected persistent stdin/stdout bridge; player
input is never interpolated into argv or a shell command. `working_directory`
is explicit. To use an agent CLI, configure its line-oriented adapter inside an
OS/container sandbox under a dedicated unprivileged account. Solaris does not
claim that spawning a child automatically creates a sandbox. Do not point it at
a privileged host shell. Child stderr is not disclosed. Session limits and pipe
backpressure are bounded; close/disconnect/revocation kills the direct child.
Sandbox launchers must supervise/reap their own descendants. No transcript or
process state is persisted. A block removed, dimension change, loss of reach,
auth lock or deop closes the session on native publication.

## Protocol 1

One Play channel `solaris:computer` in both directions. Big-endian header:
`u8 version=1, u8 kind, u32 session` (nonzero, chosen by client).
Client kinds: 0=open followed by three i32 block coordinates; 1=input followed
by UTF-8 (at most 4096 bytes, no control characters); 2=close, no body.
Server kinds: 0=opened, 1=plain UTF-8 output, 2=closed/refused; remaining bytes
are text, at most 16384. Replies retain session id; clients discard stale replies.
The channel grants no world mutation. A session is connection-bound and cannot
attach to another player's process. Native publication is the sole pose/block
observation source, not client coordinates claiming authority.

## Desktop capture spike and next stage

Choose a **small local native helper with a bounded raw RGBA frame contract**,
not Java AWT Robot as the production abstraction: Robot cannot provide native
Wayland portal selection. The Java side validates dimensions/size, owns an LWJGL
texture and uploads frames on the render thread. No capture bytes travel to the
server. Capture and keyboard/mouse forwarding require separate, explicit local
consent; merely joining a server must never start either.

Backend plan: Windows Graphics Capture (DXGI fallback); macOS ScreenCaptureKit
with OS Screen Recording consent; Linux X11 XShm/XComposite; native Wayland
xdg-desktop-portal ScreenCast + PipeWire with portal selection (never silently
capture an XWayland-only substitute). First spike uses an explicit FFmpeg x11grab
helper as an available Linux implementation. Per-OS validation and smoke receipts
are recorded with the client implementation; no other backend is claimed working
until exercised. Desktop interaction and in-world dynamic-texture rendering are
not part of the text-terminal implementation.

## Acceptance

Server codec/security/lifecycle tests; Fabric compilation and unit tests; Linux
capture-to-OpenGL-texture smoke on an isolated display, not the owner's desktop.
No real-client MCP run in this work item; Muse owns subsequent block/use/session
QA. Unit/smoke receipts do not prove in-world rendering or other operating systems.

## Screen sharing (`solaris:screen`, protocol 1)

A native client may show its captured desktop window on a `solaris:monitor`
row. By default every other player sees only a generic "monitor is on" texture
(`ACTIVE`, no frames). A viewer may ask the owner for permission; only while
the permission is active **and** the viewer looks at the row does the server
turn `DEMAND` on for the owner and relay frames to that viewer. No input from
viewers exists in the protocol. `mc-edge` (`screen.rs` pure relay state,
`screen_service.rs` delivery) is the only authority; it never decodes frames,
sees only native-Loader clients and refuses everything else.

Play custom payload on both directions, big-endian: `u8 version=1, u8 kind, body`.
`ScreenId` (16 bytes): `i32 x,y,z` of the min-coordinate anchor block, `u8 side`
(2 N, 3 S, 4 W, 5 E: the face carrying the screen), `u8 width` (1..=16 blocks,
extending +X for sides 2/3 and +Z for 4/5), `u8 height` (1..=16, extending +Y), `u8 dimension`
(`Dimension::ALL` index, must equal the sender's dimension).

Client -> server: `0 OPEN(id)` owner starts (within 8 blocks; every one of the width x height blocks must
be a monitor, i.e. a `minecraft:black_glazed_terracotta` carrier state whose
`facing` equals `side`; at most 4 per owner and 128 open screens; one owner per rectangle); `1 CLOSE(id)`;
`2 FRAME_PART(id, u32 seq, u16 w, u16 h, u8 codec=0 JPEG, u8 idx, u8 count, bytes)`
with every part carrying the full header, <=30000 data bytes per part (the
serverbound payload limit is 32767), <=5 parts, <=131072 bytes, w<=640, h<=640;
`3 REQUEST(id)` viewer asks the owner; `4 ANSWER(u32 request, u8 allow)`;
`5 REVOKE(id, 16-byte viewer uuid)`; `6 LEAVE(id)` viewer drops permission and
cancels its unanswered request to that owner (a later ANSWER grants nothing).

Server -> client: `0 DEMAND(id, u8 on, u16 max_width=640, u8 fps=5)` to the owner;
`1 FRAME(id, u32 seq, u16 w, u16 h, u8 codec, bytes)` reassembled, to viewers
only; `2 STOP(id, u8 reason)` 0 looked away, 1 owner closed, 2 revoked, 3 owner
left; `3 REFUSED(id, u8 reason)` 0 not allowed, 1 not a monitor, 2 out of reach,
3 too many, 4 taken, 5 bad frame, 6 no such screen, 7 repeat/rate limited;
`4 REQUESTED(id, u32 request, 16-byte uuid, u8 len, utf8 name)` to the owner;
`5 PERMISSION(id, u8 allowed)` to the requester; `6 ACTIVE(id, u8 on)` to every
native client in that dimension on open/close/owner leave, and as a snapshot
when a native client joins or changes dimension.
`7 AUDIO(id, u32 seq, opus)` to viewers only (see "Screen audio").

Permission is per (owner, viewer) and covers all of the owner's open screens;
it ends on REVOKE/LEAVE, when the owner closes their last screen, or when
either player disconnects. "Looking" is derived from the published native pose
only: eye position (feet + 1.62), yaw/pitch, on the face's front side, within
32 blocks and within 35 degrees plus the rectangle's angular size of its centre; no
occlusion. Reach (8 blocks) applies to OPEN only; a show ends on CLOSE, revoke,
disconnect, or when its monitor blocks are broken or unloaded (`REFUSED`). Frames are accepted only
while DEMAND is on, only from the owner, within a 1 MiB/s byte budget (256 KiB
burst); overflow frames are dropped, malformed or repeated unsolicited frames
close the screen. A viewer's full reliable queue drops the frame, never the
connection.

### Screen audio

The captured app's sound rides the same channel and the same rules as its picture.
Client -> server `7 AUDIO_PART(id, u32 seq, opus)`: one Opus packet (48 kHz mono, 20 ms,
1..=1000 bytes; the codec and jitter buffer are the voice-chat ones, ADR 0019). Server ->
client `7 AUDIO(id, u32 seq, opus)`, rewritten from the owner's part with the same id and `seq`.
The relay never decodes Opus. Audio is accepted only from the owner and only while `DEMAND`
is on; it is delivered only to granted viewers that are looking (the `watching` set that also
gates frames), so a viewer who looks away or is revoked hears nothing within one tick. It has
its own token buckets (60 packets/s with a burst of 25, 32 KiB/s with an 8 KiB burst):
overflow is dropped, never queued. Audio sent without demand is dropped and counted
separately (150 strikes close the screen as `BAD_FRAME`); an empty or >1000-byte packet closes
the screen like a malformed frame. A full reliable queue drops the packet, not the connection
(`Effect::is_media`). No new `DEMAND` field: a client captures audio exactly when it is asked
for pictures and locally approved at least one viewer.

Playback is positional: the client mixes each screen as one speaker at the monitor
rectangle's face centre with linear distance attenuation, silent beyond 32 blocks (the server's
looking range). The owner hears a nested app the same way, because its private sink is the
app's only output.

Sources, by routing rule (a capture must never contain Solaris's own playback or voice chat;
there is no echo cancellation):

- **Linux nested compositor apps**: a private PipeWire null sink per monitor (held-open `pw-cli`,
  `object.linger=false`). The app is launched with `PIPEWIRE_NODE` and `PULSE_SINK` set to it and
  `pw-cat` records the sink's monitor (48 kHz mono f32). The sink outlives every capture and is
  destroyed only after the app is killed, so a stopped capture cannot spill the app onto the
  owner's speakers. Without PipeWire tools the app simply runs without audio routing.
- **Windows**: WASAPI process loopback (`VAD\Process_Loopback`, Windows 10 2004+) of the shared
  window's process tree; our own process is refused. The app keeps playing on the owner's output,
  so the owner is not given a second positioned copy. Compile-checked for
  `x86_64-pc-windows-gnu`; **not executed on Windows**.
- **Linux portal windows**: video only. The portal offers no per-window audio and the only
  alternative is the whole-system monitor, which would loop Solaris's playback into the stream.
- Test hook: `SOLARIS_SCREEN_AUDIO_WAV` replaces the source's audio by a looped 48 kHz 16-bit
  WAV through the same capture abstraction (no behaviour change when unset).

## Monitor block (carrier-backed custom block)

Core admits one narrow Loader block shape; every other `blocks` shape is refused
at startup. A bundle with `content = [.., "blocks", ..]` and `register_blocks`
declares exactly one index entry
`{"id","name","model","carrier","drop_item"}` (all five keys, owner-namespaced
`id`/`drop_item`). `carrier` must be a core-reserved vanilla block (today only
`minecraft:black_glazed_terracotta`: full cube, horizontal `facing`, no block
entity, never generated) and `drop_item` an installed custom item whose carrier
item is that block. The world, WAL and native store persist only the carrier's
four `facing` states, so storage and vanilla clients are unchanged (they see
black glazed terracotta); a carrier state *is* the custom block. Placing the
custom item places the carrier state with vanilla facing (screen toward the
placer); the plain carrier item can no longer be placed; breaking or exploding
it drops the custom item (none in creative). Carrier states are server-dictated:
the old per-client `carrier_block_state_ids` negotiation and its dead
projection/report code are removed (a non-empty ack field is refused).
`solaris:monitor` is shipped by the first-party `computer` package. Rows of
adjacent monitors are a client rule, not core state.

Desktop input limits: with Wayland RemoteDesktop, pointer motion maps to the
captured stream, but buttons and keys are seat-wide (no target window), so the
native client fails closed when the pointer is over the game or the game has
focus. Windows uses window-targeted `PostMessage`.
