//! `solaris:voice` proximity relay (ADR 0019). Pure: no I/O and no clock;
//! callers pass `Instant`s and deliver the returned payloads. The server never
//! decodes Opus: it bounds, rate-limits and forwards each packet to listeners in
//! the same dimension within range of the speaker's published pose.
use std::{collections::HashMap, time::Instant};

use bytes::Bytes;
use mc_core::boundary::{ConnectionId, EntityId};
use serde::Deserialize;
use uuid::Uuid;

use crate::screen::Pose;

pub(crate) const CHANNEL: &str = "solaris:voice";
const VERSION: u8 = 1;
pub(crate) const MAX_OPUS_BYTES: usize = 1000;

const PACKETS_PER_SECOND: f64 = 60.0;
const PACKET_BURST: f64 = 20.0;
const BYTES_PER_SECOND: f64 = 65_536.0;
const BYTES_BURST: f64 = 16_384.0;

mod client_kind {
    pub const AUDIO: u8 = 0;
    pub const LISTEN: u8 = 1;
}
mod server_kind {
    pub const AUDIO: u8 = 0;
    pub const VOICE_STATE: u8 = 1;
}

/// `[voice]`: host-owned configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    pub enabled: bool,
    /// Hearing range in blocks, eye to eye.
    pub range_blocks: u16,
}
impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            range_blocks: 48,
        }
    }
}
impl VoiceConfig {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=1024).contains(&self.range_blocks),
            "voice.range_blocks must be 1..=1024"
        );
        Ok(())
    }
}

/// Client -> server message.
#[derive(Debug, PartialEq)]
pub(crate) enum Message<'a> {
    Audio { seq: u32, opus: &'a [u8] },
    Listen(bool),
}

pub(crate) fn decode(bytes: &[u8]) -> Option<Message<'_>> {
    let ([version, kind], body) = bytes.split_first_chunk::<2>()?;
    if *version != VERSION {
        return None;
    }
    match *kind {
        client_kind::AUDIO => {
            let (seq, opus) = body.split_first_chunk::<4>()?;
            (1..=MAX_OPUS_BYTES)
                .contains(&opus.len())
                .then(|| Message::Audio {
                    seq: u32::from_be_bytes(*seq),
                    opus,
                })
        }
        client_kind::LISTEN => match body {
            [0] => Some(Message::Listen(false)),
            [1] => Some(Message::Listen(true)),
            _ => None,
        },
        _ => None,
    }
}

/// `VOICE_STATE`: `u8 enabled, u16 range_blocks`.
pub(crate) fn voice_state(enabled: bool, range_blocks: u16) -> Bytes {
    let mut out = vec![VERSION, server_kind::VOICE_STATE, u8::from(enabled)];
    out.extend(range_blocks.to_be_bytes());
    Bytes::from(out)
}

fn audio(speaker: Uuid, eye: [f64; 3], seq: u32, opus: &[u8]) -> Bytes {
    let mut out = Vec::with_capacity(34 + opus.len());
    out.extend([VERSION, server_kind::AUDIO]);
    out.extend(speaker.as_bytes());
    for axis in eye {
        out.extend((axis as f32).to_be_bytes());
    }
    out.extend(seq.to_be_bytes());
    out.extend(opus);
    Bytes::from(out)
}

/// One payload for several connections.
#[derive(Debug, PartialEq)]
pub(crate) struct Delivery {
    pub to: Vec<ConnectionId>,
    pub payload: Bytes,
}

struct Player {
    entity: EntityId,
    uuid: Uuid,
    listening: bool,
    packets: f64,
    bytes: f64,
    updated: Instant,
}

pub(crate) struct Relay {
    enabled: bool,
    range: f64,
    range_blocks: u16,
    players: HashMap<ConnectionId, Player>,
    poses: HashMap<ConnectionId, Pose>,
}

impl Relay {
    pub(crate) fn new(config: &VoiceConfig) -> Self {
        Self {
            enabled: config.enabled,
            range: f64::from(config.range_blocks),
            range_blocks: config.range_blocks,
            players: HashMap::new(),
            poses: HashMap::new(),
        }
    }

    /// Register a native client and return its `VOICE_STATE`.
    pub(crate) fn register(
        &mut self,
        id: ConnectionId,
        entity: EntityId,
        uuid: Uuid,
        now: Instant,
    ) -> Bytes {
        self.players.insert(
            id,
            Player {
                entity,
                uuid,
                listening: false,
                packets: PACKET_BURST,
                bytes: BYTES_BURST,
                updated: now,
            },
        );
        voice_state(self.enabled, self.range_blocks)
    }

    pub(crate) fn unregister(&mut self, id: ConnectionId) {
        self.players.remove(&id);
        self.poses.remove(&id);
    }

    /// Connections and the entity whose pose `set_poses` needs.
    pub(crate) fn entities(&self) -> Vec<(ConnectionId, EntityId)> {
        self.players
            .iter()
            .map(|(id, player)| (*id, player.entity))
            .collect()
    }

    pub(crate) fn set_poses(&mut self, poses: HashMap<ConnectionId, Pose>) {
        self.poses = poses;
    }

    pub(crate) fn handle(
        &mut self,
        from: ConnectionId,
        message: Message<'_>,
        now: Instant,
    ) -> Option<Delivery> {
        if !self.enabled {
            return None;
        }
        match message {
            Message::Listen(on) => {
                self.players.get_mut(&from)?.listening = on;
                None
            }
            Message::Audio { seq, opus } => self.relay(from, seq, opus, now),
        }
    }

    fn relay(
        &mut self,
        from: ConnectionId,
        seq: u32,
        opus: &[u8],
        now: Instant,
    ) -> Option<Delivery> {
        let speaker = self.players.get_mut(&from)?;
        let elapsed = now.saturating_duration_since(speaker.updated).as_secs_f64();
        speaker.updated = now;
        speaker.packets = (speaker.packets + elapsed * PACKETS_PER_SECOND).min(PACKET_BURST);
        speaker.bytes = (speaker.bytes + elapsed * BYTES_PER_SECOND).min(BYTES_BURST);
        let cost = (opus.len() + 6) as f64;
        if speaker.packets < 1.0 || speaker.bytes < cost {
            return None;
        }
        speaker.packets -= 1.0;
        speaker.bytes -= cost;
        let uuid = speaker.uuid;
        let pose = *self.poses.get(&from)?;
        let range_squared = self.range * self.range;
        let to: Vec<ConnectionId> = self
            .players
            .iter()
            .filter(|(id, player)| **id != from && player.listening)
            .filter_map(|(id, _)| {
                let heard = self.poses.get(id)?;
                let distance_squared: f64 = (0..3)
                    .map(|axis| (heard.eye[axis] - pose.eye[axis]).powi(2))
                    .sum();
                (heard.dimension == pose.dimension && distance_squared <= range_squared)
                    .then_some(*id)
            })
            .collect();
        (!to.is_empty()).then(|| Delivery {
            to,
            payload: audio(uuid, pose.eye, seq, opus),
        })
    }
}

#[cfg(test)]
#[path = "voice_tests.rs"]
mod tests;
