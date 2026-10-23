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
