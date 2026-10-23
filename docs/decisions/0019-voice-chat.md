# ADR 0019: proximity voice chat

Status: accepted design; draft implementation, real-client QA pending.

## Decision

Native Solaris clients capture the microphone (push-to-talk), encode 20 ms mono
48 kHz Opus packets (~24 kbps) and send them to the server on the Play custom
payload channel `solaris:voice`. `mc-edge` (`voice.rs` pure relay, `voice_service.rs`
delivery) forwards each packet to the *listening* native clients that are in the
same dimension and within `range_blocks` (default 48, eye to eye) of the
speaker, excluding the speaker, attaching the speaker's uuid and position.
Receivers decode and play it spatialized. The server never decodes audio, holds
no audio state and persists nothing. Vanilla and non-native clients never see
the channel (no VOICE_STATE, no relay). Core authority is untouched: the only
inputs are poses already published by the native tick; relay runs on the
connection read task and a tick subscriber, never on the tick.

Rejected: a separate UDP voice port (second NAT/firewall surface; Play TCP
latency is adequate for a draft), server-side mixing/decoding (CPU, no need),
trusting client-claimed positions (the server uses published poses only).

## Wire (protocol 1)

Big-endian. Every payload is `u8 version=1, u8 kind, body`; unknown version/kind
or malformed body is ignored (never a kick, never an error response).

Client -> server:
- `0 AUDIO`: `u32 seq`, then 1..=1000 Opus bytes (one packet). `seq` increments
  per 20 ms frame and wraps; the server relays it verbatim. A client sends nothing
  while not talking; a receiver treats a >200 ms gap as end of speech.
- `1 LISTEN`: `u8 on` (0/1). A connection receives AUDIO only after `LISTEN(1)`
  (default off), so clients with voice off or deafened cost no bandwidth.

Server -> client (native clients only):
- `0 AUDIO`: `16B speaker uuid, f32 x, f32 y, f32 z, u32 seq, opus bytes`.
  x/y/z is the speaker's **eye** position (feet + 1.62) from the last published
  tick pose (at most one tick old).
- `1 VOICE_STATE`: `u8 enabled, u16 range_blocks`, sent once when the native
  client joins. `enabled = 0` when `[voice] enabled = false`; clients then neither
  send nor play.

## Relay rules and limits

- Listener set: registered, `LISTEN(1)`, has a published pose (alive), same
  dimension index as the speaker, squared eye distance <= `range_blocks`^2, and
  not the speaker. A speaker without a pose (dead/not yet published) is dropped.
- Per-connection token buckets (speaker side): 60 packets/s, burst 20 packets;
  64 KiB/s, burst 16 KiB (each packet charged opus length + 6). Overload drops
  the packet; it is never punished beyond that. The general play packet limiter
  (`[anti_cheat]`) also still applies to the custom-payload packet id.
- Delivery is best effort on the listener's reliable queue: a packet is dropped
  when that queue is less than half free, so audio can never starve control
  packets or retire a connection. One encoding serves all listeners of a packet.
- `[voice] enabled = true`, `range_blocks = 48` (1..=1024).

## Validation status

Unit tests (`voice_tests.rs`) cover range, dimension, self-exclusion, listening,
pose requirement, limits, wire decode and disabled mode. Real-client audio QA
(two native clients, microphone, speakers) is pending.
