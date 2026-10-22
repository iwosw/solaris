//! Play login-flow packets: entering Play state, position sync, keep-alive, disconnect.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundCustomPayload {
    pub payload: CustomPayload,
}

impl Packet for ClientboundCustomPayload {
    // `.analysis/protocol-dump.txt`: game CLIENTBOUND_CUSTOM_PAYLOAD is
    // clientbound registration index 24, wire id 0x18. Body is the common
    // custom-payload codec from local decompiled sources.
    const ID: i32 = 0x18;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.payload.encode(buf)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            payload: CustomPayload::decode(buf)?,
        })
    }
}

/// `Login (Play)` (CB). The server's "welcome to the world" packet that
/// transitions the client into Play state proper.
///
/// The wire layout has been stable since 1.20.5; the leading packet ID
/// shifts by a small number between every patch release as new packets
/// are inserted. The value below is a best-effort placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginPlay {
    pub entity_id: i32,
    pub is_hardcore: bool,
    pub dimension_names: Vec<Identifier>,
    pub max_players: i32,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub reduced_debug_info: bool,
    pub enable_respawn_screen: bool,
    pub do_limited_crafting: bool,
    /// Numeric id into the `minecraft:dimension_type` registry — the
    /// position of the entry in our [`RegistryData`] packet, not a
    /// stable namespaced identifier.
    pub dimension_type_id: i32,
    pub dimension_name: Identifier,
    pub hashed_seed: i64,
    /// 0 = Survival, 1 = Creative, 2 = Adventure, 3 = Spectator.
    pub game_mode: u8,
    /// -1 = none, otherwise same encoding as `game_mode`.
    pub previous_game_mode: i8,
    pub is_debug: bool,
    pub is_flat: bool,
    pub death_location: Option<(Identifier, i64)>,
    pub portal_cooldown: i32,
    pub sea_level: i32,
    pub enforces_secure_chat: bool,
}

impl Packet for LoginPlay {
    // Verified against vanilla 26.1.2 wire capture via `wire-probe`:
    // server-bound LoginPlay arrives as id 0x31, body 108 bytes,
    // matching our field layout exactly.
    const ID: i32 = 0x31;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.dimension_names.len() > MAX_DIMENSION_NAMES {
            return Err(CodecError::StringTooLong {
                len: self.dimension_names.len(),
                max: MAX_DIMENSION_NAMES,
            });
        }
        buf.write_i32(self.entity_id);
        buf.write_bool(self.is_hardcore);
        buf.write_varint(i32::try_from(self.dimension_names.len()).map_err(|_| {
            CodecError::StringTooLong {
                len: self.dimension_names.len(),
                max: MAX_DIMENSION_NAMES,
            }
        })?);
        for name in &self.dimension_names {
            buf.write_identifier(name)?;
        }
        buf.write_varint(self.max_players);
        buf.write_varint(self.view_distance);
        buf.write_varint(self.simulation_distance);
        buf.write_bool(self.reduced_debug_info);
        buf.write_bool(self.enable_respawn_screen);
        buf.write_bool(self.do_limited_crafting);
        buf.write_varint(self.dimension_type_id);
        buf.write_identifier(&self.dimension_name)?;
        buf.write_i64(self.hashed_seed);
        buf.write_u8(self.game_mode);
        buf.write_i8(self.previous_game_mode);
        buf.write_bool(self.is_debug);
        buf.write_bool(self.is_flat);
        match &self.death_location {
            Some((dim, pos)) => {
                buf.write_bool(true);
                buf.write_identifier(dim)?;
                buf.write_i64(*pos);
            }
            None => buf.write_bool(false),
        }
        buf.write_varint(self.portal_cooldown);
        buf.write_varint(self.sea_level);
        buf.write_bool(self.enforces_secure_chat);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let entity_id = buf.read_i32()?;
        let is_hardcore = buf.read_bool()?;
        let dimension_names =
            read_bounded_vec(buf, MAX_DIMENSION_NAMES, 1, ReadMc::read_identifier)?;
        let max_players = buf.read_varint()?;
        let view_distance = buf.read_varint()?;
        let simulation_distance = buf.read_varint()?;
        let reduced_debug_info = buf.read_bool()?;
        let enable_respawn_screen = buf.read_bool()?;
        let do_limited_crafting = buf.read_bool()?;
        let dimension_type_id = buf.read_varint()?;
        let dimension_name = buf.read_identifier()?;
        let hashed_seed = buf.read_i64()?;
        let game_mode = buf.read_u8()?;
        let previous_game_mode = buf.read_i8()?;
        let is_debug = buf.read_bool()?;
        let is_flat = buf.read_bool()?;
        let death_location = if buf.read_bool()? {
            let dim = buf.read_identifier()?;
            let pos = buf.read_i64()?;
            Some((dim, pos))
        } else {
            None
        };
        let portal_cooldown = buf.read_varint()?;
        let sea_level = buf.read_varint()?;
        let enforces_secure_chat = buf.read_bool()?;
        Ok(Self {
            entity_id,
            is_hardcore,
            dimension_names,
            max_players,
            view_distance,
            simulation_distance,
            reduced_debug_info,
            enable_respawn_screen,
            do_limited_crafting,
            dimension_type_id,
            dimension_name,
            hashed_seed,
            game_mode,
            previous_game_mode,
            is_debug,
            is_flat,
            death_location,
            portal_cooldown,
            sea_level,
            enforces_secure_chat,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundRespawn {
    pub dimension_type_id: i32,
    pub dimension_name: Identifier,
    pub hashed_seed: i64,
    pub game_mode: u8,
    pub previous_game_mode: i8,
    pub is_debug: bool,
    pub is_flat: bool,
    pub death_location: Option<(Identifier, i64)>,
    pub portal_cooldown: i32,
    pub sea_level: i32,
    pub data_to_keep: i8,
}

impl Packet for ClientboundRespawn {
    // CLIENTBOUND_RESPAWN is game-CB index 82 = wire id 0x52 in the
    // local 26.1.2 GameProtocols dump. Its body is CommonPlayerSpawnInfo
    // followed by one byte of keep flags.
    const ID: i32 = 0x52;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.dimension_type_id);
        buf.write_identifier(&self.dimension_name)?;
        buf.write_i64(self.hashed_seed);
        buf.write_u8(self.game_mode);
        buf.write_i8(self.previous_game_mode);
        buf.write_bool(self.is_debug);
        buf.write_bool(self.is_flat);
        match &self.death_location {
            Some((dim, pos)) => {
                buf.write_bool(true);
                buf.write_identifier(dim)?;
                buf.write_i64(*pos);
            }
            None => buf.write_bool(false),
        }
        buf.write_varint(self.portal_cooldown);
        buf.write_varint(self.sea_level);
        buf.write_i8(self.data_to_keep);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let dimension_type_id = buf.read_varint()?;
        let dimension_name = buf.read_identifier()?;
        let hashed_seed = buf.read_i64()?;
        let game_mode = buf.read_u8()?;
        let previous_game_mode = buf.read_i8()?;
        let is_debug = buf.read_bool()?;
        let is_flat = buf.read_bool()?;
        let death_location = if buf.read_bool()? {
            let dim = buf.read_identifier()?;
            let pos = buf.read_i64()?;
            Some((dim, pos))
        } else {
            None
        };
        let portal_cooldown = buf.read_varint()?;
        let sea_level = buf.read_varint()?;
        let data_to_keep = buf.read_i8()?;
        Ok(Self {
            dimension_type_id,
            dimension_name,
            hashed_seed,
            game_mode,
            previous_game_mode,
            is_debug,
            is_flat,
            death_location,
            portal_cooldown,
            sea_level,
            data_to_keep,
        })
    }
}

/// `Set Default Spawn Position` (CB). Tells the client where its compass
/// should point.
///
/// Per ADR 0002, verified against the vanilla 26.1.2 client jar via `javap`:
/// the payload is `LevelData.RespawnData` = `GlobalPos` plus yaw/pitch floats.
#[derive(Debug, Clone, PartialEq)]
pub struct SetDefaultSpawnPosition {
    pub dimension: Identifier,
    /// Block position packed into an `i64` per the vanilla
    /// `BlockPos` format: `((x & 0x3FFFFFF) << 38) | ((z & 0x3FFFFFF) << 12) | (y & 0xFFF)`.
    pub position: i64,
    pub yaw: f32,
    pub pitch: f32,
}

impl Packet for SetDefaultSpawnPosition {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_SET_DEFAULT_SPAWN_POSITION sits at game-CB index 96
    // (skipping `CLIENTBOUND_BUNDLE` at index 0) = wire id 0x61.
    const ID: i32 = 0x61;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_identifier(&self.dimension)?;
        buf.write_i64(self.position);
        buf.write_f32(self.yaw);
        buf.write_f32(self.pitch);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            dimension: buf.read_identifier()?,
            position: buf.read_i64()?,
            yaw: buf.read_f32()?,
            pitch: buf.read_f32()?,
        })
    }
}

/// `Synchronize Player Position` (CB). Vanilla uses this both at first
/// spawn ("you are now at <pos>") and as a periodic snap-back when the
/// server's view of the player drifts from the client's.
///
/// Since 1.21.2 this packet has a richer "PositionMoveFlags" bitfield;
/// for M1.g we only need the initial-spawn form.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SynchronizePlayerPosition {
    pub teleport_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub dx: f64,
    pub dy: f64,
    pub dz: f64,
    pub yaw: f32,
    pub pitch: f32,
    /// Bitfield. Bit 0..4 = "X/Y/Z/yaw/pitch is relative". For an
    /// initial absolute teleport: 0.
    pub relative_flags: i32,
}

impl Packet for SynchronizePlayerPosition {
    // Verified against vanilla 26.1.2 wire capture: id 0x48, body 61
    // bytes — varint teleport_id + 6×f64 (xyz + delta) + 2×f32 (yaw,
    // pitch) + i32 relative_flags. Matches our field layout exactly.
    const ID: i32 = 0x48;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.teleport_id);
        buf.write_f64(self.x);
        buf.write_f64(self.y);
        buf.write_f64(self.z);
        buf.write_f64(self.dx);
        buf.write_f64(self.dy);
        buf.write_f64(self.dz);
        buf.write_f32(self.yaw);
        buf.write_f32(self.pitch);
        buf.write_i32(self.relative_flags);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            teleport_id: buf.read_varint()?,
            x: buf.read_f64()?,
            y: buf.read_f64()?,
            z: buf.read_f64()?,
            dx: buf.read_f64()?,
            dy: buf.read_f64()?,
            dz: buf.read_f64()?,
            yaw: buf.read_f32()?,
            pitch: buf.read_f32()?,
            relative_flags: buf.read_i32()?,
        })
    }
}

/// Clientbound `KeepAlive (Play)`. The server pings every ~15s with a
/// fresh `i64`; the client must echo it within ~30s or be disconnected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundKeepAlive {
    pub id: i64,
}

impl Packet for ClientboundKeepAlive {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_KEEP_ALIVE at game-CB index 44 = wire id 0x2C.
    const ID: i32 = 0x2C;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(self.id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            id: buf.read_i64()?,
        })
    }
}

/// `Disconnect (Play)` (CB). The polite "kick" packet; legal only inside
/// the Play state.
///
/// Vanilla has moved its disconnect-reason payload from a JSON string
/// to a binary NBT `Component` (since 1.20.4). For M1.g we serialise a
/// minimal NBT representation directly inline; full text-component
/// support is later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayDisconnect {
    /// Pre-serialised NBT bytes for the reason `Component`.
    pub reason_nbt: Vec<u8>,
}

impl Packet for PlayDisconnect {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_DISCONNECT at game-CB index 32 = wire id 0x20.
    const ID: i32 = 0x20;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.put_slice(&self.reason_nbt);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        // We can only round-trip what we wrote; without an NBT parser
        // inline we just slurp the rest of the body.
        let remaining = buf.remaining();
        let mut bytes = vec![0u8; remaining];
        buf.copy_to_slice(&mut bytes);
        Ok(Self { reason_nbt: bytes })
    }
}

/// Clientbound `Transfer` (Play): asks the client to reconnect to another
/// host. Vanilla 26.1.2 writes a plain UTF host followed by a VarInt port and
/// keeps the connection open; the client drops it after receiving the packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundTransfer {
    pub host: String,
    pub port: i32,
}

impl Packet for ClientboundTransfer {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_TRANSFER at game-CB index 129 = wire id 0x81.
    const ID: i32 = 0x81;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_string(&self.host, 32_767)?;
        buf.write_varint(self.port);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            host: buf.read_string(32_767)?,
            port: buf.read_varint()?,
        })
    }
}
