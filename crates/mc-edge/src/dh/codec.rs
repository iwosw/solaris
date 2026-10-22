//! Distant Horizons protocol-16 message codec.
//!
//! Independent implementation of the Play custom-payload framing described in
//! `.analysis/DHCompat/design.md` sections 2-3. No upstream code was read or
//! copied here. Envelope: `u16be protocol (=16), u16be message type, body`.
//! DH strings are a u16 **byte** length followed by UTF-8 bytes (not MC
//! VarInt strings); integers are fixed-width big-endian; booleans are a
//! single byte; request timestamps are boolean-present plus an optional value.
//!
//! Fail-closed rules: a non-16 protocol gets a dedicated understandable error
//! (ordinary Minecraft play must stay usable for other clients); unknown
//! types, truncation and trailing bytes are rejected; every length is checked
//! against the remaining input before any allocation; beacons are validated
//! and skipped (encode always writes an empty beacon collection).

use anyhow::Result;

/// DH network protocol this codec speaks. Anything else is rejected with a
/// dedicated error, never silently decoded.
pub(crate) const PROTOCOL: u16 = 16;

/// Generator-plan ordinals, protocol-16 SessionConfig order.
pub(crate) const PLAN_SURFACE_THEN_CHUNKS: u8 = 0;
#[cfg(test)]
pub(crate) const PLAN_SURFACE_ONLY: u8 = 1;
pub(crate) const PLAN_CHUNKS_ONLY: u8 = 2;
pub(crate) const PLAN_DISABLED: u8 = 3;

/// Finest servable section detail: a detail-6 section covers 64x64 blocks.
/// Coarser requests (detail > 6) pass this codec untouched; the session
/// answers them with [`ERR_NEEDS_SPLIT`].
pub(crate) const LEAF_DETAIL: u8 = 6;

/// World half-extent in blocks used for section bounds checks.
pub(crate) const WORLD_LIMIT: i32 = 30_000_000;

/// `Exception` error-kind indices.
pub(crate) const ERR_RATE_LIMITED: i32 = 0;
pub(crate) const ERR_OUT_OF_RANGE: i32 = 1;
pub(crate) const ERR_REJECTED: i32 = 2;
/// The requested section needs splitting (used for detail > [`LEAF_DETAIL`]).
pub(crate) const ERR_NEEDS_SPLIT: i32 = 3;

const TYPE_CLOSE: u16 = 1;
const TYPE_LEVEL_INIT: u16 = 2;
const TYPE_REQUEST_LEVEL_INIT: u16 = 3;
const TYPE_CONFIG: u16 = 4;
const TYPE_CANCEL: u16 = 5;
const TYPE_EXCEPTION: u16 = 6;
const TYPE_REQUEST: u16 = 7;
const TYPE_RESPONSE: u16 = 8;
const TYPE_UPDATE: u16 = 9;
const TYPE_SPLIT: u16 = 10;

/// One DH protocol-16 message, envelope excluded from the variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Message {
    /// Type 1: DH session close reason (not a Minecraft disconnect).
    Close(String),
    /// Type 2: server level identity + server clock for timestamp comparison.
    LevelInit {
        dimension: String,
        server_key: String,
        level_key: String,
        time: i64,
    },
    /// Type 3: client asks for the level init of a dimension.
    RequestLevelInit(String),
    /// Type 4: session config, fixed 39-byte body, no names/counts.
    Config(Config),
    /// Type 5: cancel a trackable future by its 32-bit id.
    Cancel(i32),
    /// Type 6: trackable failure; kind is an [`ERR_*`] index.
    Exception { id: i32, kind: i32, message: String },
    /// Type 7: full-data request; absent timestamp = generate, present = sync.
    Request {
        id: i32,
        level: String,
        position: i64,
        timestamp: Option<i64>,
    },
    /// Type 8: final descriptor for a request; `buffer` is the DTO buffer id.
    /// Beacon entries are skipped on decode, always empty on encode.
    Response { id: i32, buffer: Option<i32> },
    /// Type 9: real-time partial update for a level key.
    Update { level: String, buffer: i32 },
    /// Type 10: one DTO fragment; `first` marks the first frame of a buffer.
    Split {
        buffer: i32,
        data: Vec<u8>,
        first: bool,
    },
}

/// Protocol-16 SessionConfig. Field order on the wire is declaration order;
/// the body is always 39 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    pub plan: u8,
    pub distance: i32,
    pub center_x: i32,
    pub center_z: i32,
    pub radius: i32,
    pub requests: i32,
    pub updates: bool,
    pub update_radius: i32,
    pub sync: bool,
    pub sync_distance: i32,
    pub sync_requests: i32,
    pub bandwidth: i32,
}

/// A DH packed section position: low 8-bit detail field, then signed 28-bit
/// X at bit 8 and signed 28-bit Z at bit 36. At detail 6 the section covers
/// `64x64` blocks, so the leaf block origin is `(x * 64, z * 64)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Section {
    pub detail: u8,
    pub x: i32,
    pub z: i32,
}

/// Encode a message including the protocol + type envelope.
pub(crate) fn encode(msg: &Message) -> Result<Vec<u8>> {
    let mut w = Writer::new();
    w.u16(PROTOCOL);
    match msg {
        Message::Close(reason) => {
            w.u16(TYPE_CLOSE);
            w.dh_string(reason)?;
        }
        Message::LevelInit {
            dimension,
            server_key,
            level_key,
            time,
        } => {
            w.u16(TYPE_LEVEL_INIT);
            w.dh_string(dimension)?;
            w.dh_string(server_key)?;
            w.dh_string(level_key)?;
            w.i64(*time);
        }
        Message::RequestLevelInit(dimension) => {
            w.u16(TYPE_REQUEST_LEVEL_INIT);
            w.dh_string(dimension)?;
        }
        Message::Config(config) => {
            w.u16(TYPE_CONFIG);
            config.validate()?;
            w.u8(config.plan);
            w.i32(config.distance);
            w.i32(config.center_x);
            w.i32(config.center_z);
            w.i32(config.radius);
            w.i32(config.requests);
            w.bool(config.updates);
            w.i32(config.update_radius);
            w.bool(config.sync);
            w.i32(config.sync_distance);
            w.i32(config.sync_requests);
            w.i32(config.bandwidth);
        }
        Message::Cancel(id) => {
            w.u16(TYPE_CANCEL);
            w.i32(*id);
        }
        Message::Exception { id, kind, message } => {
            w.u16(TYPE_EXCEPTION);
            w.i32(*id);
            w.i32(*kind);
            w.dh_string(message)?;
        }
        Message::Request {
            id,
            level,
            position,
            timestamp,
        } => {
            w.u16(TYPE_REQUEST);
            w.i32(*id);
            w.dh_string(level)?;
            w.i64(*position);
            w.bool(timestamp.is_some());
            if let Some(ts) = timestamp {
                w.i64(*ts);
            }
        }
        Message::Response { id, buffer } => {
            w.u16(TYPE_RESPONSE);
            w.i32(*id);
            w.bool(buffer.is_some());
            if let Some(buffer) = buffer {
                w.i32(*buffer);
                // Empty beacon collection: valid when there are no beams.
                w.i32(0);
            }
        }
        Message::Update { level, buffer } => {
            w.u16(TYPE_UPDATE);
            w.dh_string(level)?;
            w.i32(*buffer);
            w.i32(0);
        }
        Message::Split {
            buffer,
            data,
            first,
        } => {
            w.u16(TYPE_SPLIT);
            w.i32(*buffer);
            if data.len() > i32::MAX as usize {
                anyhow::bail!("DH split data too large: {} bytes", data.len());
            }
            w.i32(data.len() as i32);
            w.bytes(data);
            w.bool(*first);
        }
    }
    Ok(w.finish())
}

/// Decode one message; the whole slice must be exactly one enveloped message.
pub(crate) fn decode(buf: &[u8]) -> Result<Message> {
    let mut r = Reader::new(buf);
    if r.remaining() < 4 {
        anyhow::bail!(
            "DH message too short: {} bytes, need at least 4 for protocol + type",
            r.remaining()
        );
    }
    let protocol = r.u16()?;
    if protocol != PROTOCOL {
        anyhow::bail!(
            "unsupported DH protocol {protocol}: this codec speaks protocol {PROTOCOL} only; \
             leave ordinary Minecraft play usable for other clients"
        );
    }
    let ty = r.u16()?;
    let msg = match ty {
        TYPE_CLOSE => Message::Close(r.dh_string()?),
        TYPE_LEVEL_INIT => Message::LevelInit {
            dimension: r.dh_string()?,
            server_key: r.dh_string()?,
            level_key: r.dh_string()?,
            time: r.i64()?,
        },
        TYPE_REQUEST_LEVEL_INIT => Message::RequestLevelInit(r.dh_string()?),
        TYPE_CONFIG => {
            let config = Config {
                plan: r.u8()?,
                distance: r.i32()?,
                center_x: r.i32()?,
                center_z: r.i32()?,
                radius: r.i32()?,
                requests: r.i32()?,
                updates: r.bool()?,
                update_radius: r.i32()?,
                sync: r.bool()?,
                sync_distance: r.i32()?,
                sync_requests: r.i32()?,
                bandwidth: r.i32()?,
            };
            config.validate()?;
            Message::Config(config)
        }
        TYPE_CANCEL => Message::Cancel(r.i32()?),
        TYPE_EXCEPTION => Message::Exception {
            id: r.i32()?,
            kind: r.i32()?,
            message: r.dh_string()?,
        },
        TYPE_REQUEST => {
            let id = r.i32()?;
            let level = r.dh_string()?;
            let position = r.i64()?;
            let has_timestamp = r.bool()?;
            let timestamp = if has_timestamp { Some(r.i64()?) } else { None };
            Message::Request {
                id,
                level,
                position,
                timestamp,
            }
        }
        TYPE_RESPONSE => {
            let id = r.i32()?;
            let present = r.bool()?;
            let buffer = if present {
                let buffer = r.i32()?;
                skip_beacons(&mut r)?;
                Some(buffer)
            } else {
                None
            };
            Message::Response { id, buffer }
        }
        TYPE_UPDATE => {
            let level = r.dh_string()?;
            let buffer = r.i32()?;
            skip_beacons(&mut r)?;
            Message::Update { level, buffer }
        }
        TYPE_SPLIT => {
            let buffer = r.i32()?;
            let len = r.i32()?;
            if len < 0 {
                anyhow::bail!("invalid DH split length {len}");
            }
            let data = r.take(len as usize)?.to_vec();
            let first = r.bool()?;
            Message::Split {
                buffer,
                data,
                first,
            }
        }
        _ => anyhow::bail!("unknown DH message type {ty} for protocol {PROTOCOL}"),
    };
    r.finish(ty)?;
    Ok(msg)
}

impl Config {
    /// Reject unknown plans and negative limits before any config work.
    /// Bandwidth 0 means unlimited; radius 0 means unlimited.
    pub(crate) fn validate(&self) -> Result<()> {
        if self.plan > PLAN_DISABLED {
            anyhow::bail!("unknown DH generator plan {}", self.plan);
        }
        let limits = [
            ("distance", self.distance),
            ("radius", self.radius),
            ("requests", self.requests),
            ("update_radius", self.update_radius),
            ("sync_distance", self.sync_distance),
            ("sync_requests", self.sync_requests),
            ("bandwidth", self.bandwidth),
        ];
        for (name, value) in limits {
            if value < 0 {
                anyhow::bail!("DH config {name} must be >= 0, got {value}");
            }
        }
        Ok(())
    }

    /// Constrain a client offer against the server config: plan intersection,
    /// minimum of distances/rates, AND of flags, server-owned center/radius,
    /// and the lower nonzero bandwidth cap (zero only if both unlimited).
    /// Both sides must have passed [`Config::validate`].
    pub(crate) fn constrain(&self, client: &Self) -> Self {
        Self {
            plan: intersect_plan(self.plan, client.plan),
            distance: self.distance.min(client.distance),
            center_x: self.center_x,
            center_z: self.center_z,
            radius: self.radius,
            requests: self.requests.min(client.requests),
            updates: self.updates && client.updates,
            update_radius: self.update_radius.min(client.update_radius),
            sync: self.sync && client.sync,
            sync_distance: self.sync_distance.min(client.sync_distance),
            sync_requests: self.sync_requests.min(client.sync_requests),
            bandwidth: lower_nonzero_bandwidth(self.bandwidth, client.bandwidth),
        }
    }

    /// Validated [`Config::constrain`]: rejects negative limits or unknown
    /// plans on either side before doing config work.
    #[cfg(test)]
    pub(crate) fn try_constrain(&self, client: &Self) -> Result<Self> {
        self.validate()?;
        client.validate()?;
        Ok(self.constrain(client))
    }
}

fn intersect_plan(server: u8, client: u8) -> u8 {
    if server == client {
        return server;
    }
    if server == PLAN_DISABLED || client == PLAN_DISABLED {
        return PLAN_DISABLED;
    }
    if server > PLAN_DISABLED || client > PLAN_DISABLED {
        return PLAN_DISABLED;
    }
    if server == PLAN_SURFACE_THEN_CHUNKS {
        return client;
    }
    if client == PLAN_SURFACE_THEN_CHUNKS {
        return server;
    }
    // Conflicting restricted plans use the server's override.
    server
}

fn lower_nonzero_bandwidth(server: i32, client: i32) -> i32 {
    if server == 0 {
        return client;
    }
    if client == 0 {
        return server;
    }
    server.min(client)
}

impl Section {
    /// Split a packed position into detail + signed 28-bit coordinates.
    /// The full 8-bit detail range decodes here; only [`LEAF_DETAIL`]
    /// leaves are servable, coarser sections are the session's split case.
    pub(crate) fn decode(packed: i64) -> Result<Self> {
        let detail = (packed & 0xFF) as u8;
        let x = sign_extend_28(((packed >> 8) & 0x0FFF_FFFF) as i32);
        let z = sign_extend_28(((packed >> 36) & 0x0FFF_FFFF) as i32);
        Ok(Self { detail, x, z })
    }

    /// Pack a position using Java's 7-bit detail mask and 28-bit coordinates.
    pub(crate) fn packed(&self) -> i64 {
        (i64::from(self.detail) & 0x7F)
            | ((i64::from(self.x) & 0x0FFF_FFFF) << 8)
            | ((i64::from(self.z) & 0x0FFF_FFFF) << 36)
    }

    /// Block bounds of this section as `([min_x, min_z], [max_x, max_z])`
    /// with an **inclusive** max: leaf `(detail 6, x: 0, z: 0)` spans
    /// `([0, 0], [63, 63])`, matching upstream min/max block distance.
    pub(crate) fn bounds(&self) -> Result<([i32; 2], [i32; 2])> {
        if self.detail >= 25 {
            anyhow::bail!("DH section detail {} exceeds world bounds", self.detail);
        }
        let width = 1i64.checked_shl(u32::from(self.detail)).ok_or_else(|| {
            anyhow::anyhow!("DH section detail {} overflows block bounds", self.detail)
        })?;
        let min_x = i64::from(self.x)
            .checked_mul(width)
            .ok_or_else(|| anyhow::anyhow!("DH section x {} overflows", self.x))?;
        let min_z = i64::from(self.z)
            .checked_mul(width)
            .ok_or_else(|| anyhow::anyhow!("DH section z {} overflows", self.z))?;
        let max_x = min_x + width - 1;
        let max_z = min_z + width - 1;
        let limit = i64::from(WORLD_LIMIT);
        if min_x < -limit || max_x > limit || min_z < -limit || max_z > limit {
            anyhow::bail!(
                "DH section (detail {}, x {}, z {}) block bounds \
                 [{min_x}..{max_x}] x [{min_z}..{max_z}] outside +-{WORLD_LIMIT} world",
                self.detail,
                self.x,
                self.z,
            );
        }
        Ok(([min_x as i32, min_z as i32], [max_x as i32, max_z as i32]))
    }
}

fn sign_extend_28(v: i32) -> i32 {
    if v & 0x0800_0000 != 0 {
        v | -0x1000_0000
    } else {
        v
    }
}

/// Validate and skip a beacon collection (i32 count + 16 bytes per beacon:
/// three i32 coordinates + i32 color). Bounds-checked before advancing.
fn skip_beacons(r: &mut Reader<'_>) -> Result<()> {
    let count = r.i32()?;
    if count < 0 {
        anyhow::bail!("invalid DH beacon count {count}");
    }
    let need = (count as usize)
        .checked_mul(16)
        .ok_or_else(|| anyhow::anyhow!("DH beacon count {count} overflows"))?;
    if need > r.remaining() {
        anyhow::bail!(
            "DH beacon entries truncated: count {count} needs {need} bytes, only {} left",
            r.remaining()
        );
    }
    let _ = r.take(need)?;
    Ok(())
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            anyhow::bail!(
                "DH message truncated: need {n} bytes, only {} available",
                self.remaining()
            );
        }
        let slice = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            b => anyhow::bail!("invalid DH boolean value {b}: expected 0 or 1"),
        }
    }

    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn i32(&mut self) -> Result<i32> {
        let b = self.take(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i64(&mut self) -> Result<i64> {
        let b = self.take(8)?;
        Ok(i64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn dh_string(&mut self) -> Result<String> {
        let len = self.u16()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|e| anyhow::anyhow!("invalid DH string: not UTF-8 ({e})"))
    }

    fn finish(&self, ty: u16) -> Result<()> {
        if self.remaining() != 0 {
            anyhow::bail!(
                "DH message type {ty} has {} trailing bytes: exact consumption required",
                self.remaining()
            );
        }
        Ok(())
    }
}

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }

    fn finish(self) -> Vec<u8> {
        self.buf
    }

    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn bool(&mut self, v: bool) {
        self.buf.push(u8::from(v));
    }

    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    fn bytes(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }

    fn dh_string(&mut self, v: &str) -> Result<()> {
        if v.len() > u16::MAX as usize {
            anyhow::bail!("DH string too long: {} bytes", v.len());
        }
        self.u16(v.len() as u16);
        self.bytes(v.as_bytes());
        Ok(())
    }
}

#[cfg(test)]
#[path = "codec_tests.rs"]
mod codec_tests;
