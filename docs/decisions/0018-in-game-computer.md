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
`ScreenId` (15 bytes): `i32 x,y,z` of the min-coordinate anchor block, `u8 side`
(2 N, 3 S, 4 W, 5 E: the face carrying the screen), `u8 width` (1..=16 blocks,
extending +X for sides 2/3 and +Z for 4/5, one block high), `u8 dimension`
(`Dimension::ALL` index, must equal the sender's dimension).

Client -> server: `0 OPEN(id)` owner starts (within 8 blocks; every row block must
be a monitor, i.e. a `minecraft:black_glazed_terracotta` carrier state whose
`facing` equals `side`; at most 4 per owner and 128 open screens; one owner per row); `1 CLOSE(id)`;
`2 FRAME_PART(id, u32 seq, u16 w, u16 h, u8 codec=0 JPEG, u8 idx, u8 count, bytes)`
with every part carrying the full header, <=30000 data bytes per part (the
serverbound payload limit is 32767), <=5 parts, <=131072 bytes, w<=640, h<=640;
`3 REQUEST(id)` viewer asks the owner; `4 ANSWER(u32 request, u8 allow)`;
`5 REVOKE(id, 16-byte viewer uuid)`; `6 LEAVE(id)` viewer drops permission.

Server -> client: `0 DEMAND(id, u8 on, u16 max_width=640, u8 fps=5)` to the owner;
`1 FRAME(id, u32 seq, u16 w, u16 h, u8 codec, bytes)` reassembled, to viewers
only; `2 STOP(id, u8 reason)` 0 looked away, 1 owner closed, 2 revoked, 3 owner
left; `3 REFUSED(id, u8 reason)` 0 not allowed, 1 not a monitor, 2 out of reach,
3 too many, 4 taken, 5 bad frame, 6 no such screen, 7 repeat/rate limited;
`4 REQUESTED(id, u32 request, 16-byte uuid, u8 len, utf8 name)` to the owner;
`5 PERMISSION(id, u8 allowed)` to the requester; `6 ACTIVE(id, u8 on)` to every
native client in that dimension on open/close/owner leave, and as a snapshot
when a native client joins or changes dimension.

Permission is per (owner, viewer) and covers all of the owner's open screens;
it ends on REVOKE/LEAVE, when the owner closes their last screen, or when
either player disconnects. "Looking" is derived from the published native pose
only: eye position (feet + 1.62), yaw/pitch, on the face's front side, within
32 blocks and within 35 degrees plus the row's angular size of its centre; no
occlusion. Reach (8 blocks) applies to OPEN only; a show ends on CLOSE, revoke,
disconnect, or when its monitor blocks are broken or unloaded (`REFUSED`). Frames are accepted only
while DEMAND is on, only from the owner, within a 1 MiB/s byte budget (256 KiB
burst); overflow frames are dropped, malformed or repeated unsolicited frames
close the screen. A viewer's full reliable queue drops the frame, never the
connection.
