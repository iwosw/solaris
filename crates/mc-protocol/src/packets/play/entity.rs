//! Play entity packets: entity sync, metadata, movement, passengers, equipment-adjacent updates.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityVec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl EntityVec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionMoveRotation {
    pub position: EntityVec3,
    pub delta_movement: EntityVec3,
    pub yaw: f32,
    pub pitch: f32,
}

impl PositionMoveRotation {
    fn encode<B: BufMut>(&self, buf: &mut B) {
        write_vec3(buf, self.position);
        write_vec3(buf, self.delta_movement);
        buf.write_f32(self.yaw);
        buf.write_f32(self.pitch);
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            position: read_vec3(buf)?,
            delta_movement: read_vec3(buf)?,
            yaw: buf.read_f32()?,
            pitch: buf.read_f32()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerInfoActions(pub(super) u8);

impl PlayerInfoActions {
    pub const ADD_PLAYER: Self = Self(1 << 0);
    pub const INITIALIZE_CHAT: Self = Self(1 << 1);
    pub const UPDATE_GAME_MODE: Self = Self(1 << 2);
    pub const UPDATE_LISTED: Self = Self(1 << 3);
    pub const UPDATE_LATENCY: Self = Self(1 << 4);
    pub const UPDATE_DISPLAY_NAME: Self = Self(1 << 5);
    pub const UPDATE_LIST_ORDER: Self = Self(1 << 6);
    pub const UPDATE_HAT: Self = Self(1 << 7);

    pub const fn minimal_add_player() -> Self {
        Self(
            Self::ADD_PLAYER.0
                | Self::UPDATE_GAME_MODE.0
                | Self::UPDATE_LISTED.0
                | Self::UPDATE_LATENCY.0
                | Self::UPDATE_DISPLAY_NAME.0
                | Self::UPDATE_LIST_ORDER.0
                | Self::UPDATE_HAT.0,
        )
    }

    fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfoEntry {
    pub profile_id: Uuid,
    pub name: String,
    pub properties: Vec<GameProfileProperty>,
    pub listed: bool,
    pub latency: i32,
    pub game_mode: i32,
    pub list_order: i32,
    pub show_hat: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfoUpdate {
    pub actions: PlayerInfoActions,
    pub entries: Vec<PlayerInfoEntry>,
}

impl Packet for PlayerInfoUpdate {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_PLAYER_INFO_UPDATE
    // sits at game-CB index 70 = wire id 0x46. `javap` shows an 8-bit fixed
    // EnumSet, then a VarInt-counted entry list keyed by profile UUID.
    const ID: i32 = 0x46;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_u8(self.actions.0);
        write_count(buf, self.entries.len())?;
        for entry in &self.entries {
            buf.write_uuid(entry.profile_id);
            if self.actions.contains(PlayerInfoActions::ADD_PLAYER) {
                buf.write_string(&entry.name, 16)?;
                if entry.properties.len() > MAX_GAME_PROFILE_PROPERTIES {
                    return Err(CodecError::StringTooLong {
                        len: entry.properties.len(),
                        max: MAX_GAME_PROFILE_PROPERTIES,
                    });
                }
                write_count(buf, entry.properties.len())?;
                for property in &entry.properties {
                    buf.write_string(&property.name, MAX_GAME_PROFILE_PROPERTY_NAME_LEN)?;
                    buf.write_string(&property.value, MAX_GAME_PROFILE_PROPERTY_VALUE_LEN)?;
                    match &property.signature {
                        Some(signature) => {
                            buf.write_bool(true);
                            buf.write_string(signature, MAX_GAME_PROFILE_PROPERTY_SIGNATURE_LEN)?;
                        }
                        None => buf.write_bool(false),
                    }
                }
            }
            if self.actions.contains(PlayerInfoActions::INITIALIZE_CHAT) {
                return Err(CodecError::NotSupported("player chat session data"));
            }
            if self.actions.contains(PlayerInfoActions::UPDATE_GAME_MODE) {
                buf.write_varint(entry.game_mode);
            }
            if self.actions.contains(PlayerInfoActions::UPDATE_LISTED) {
                buf.write_bool(entry.listed);
            }
            if self.actions.contains(PlayerInfoActions::UPDATE_LATENCY) {
                buf.write_varint(entry.latency);
            }
            if self
                .actions
                .contains(PlayerInfoActions::UPDATE_DISPLAY_NAME)
            {
                buf.write_bool(false);
            }
            if self.actions.contains(PlayerInfoActions::UPDATE_LIST_ORDER) {
                buf.write_varint(entry.list_order);
            }
            if self.actions.contains(PlayerInfoActions::UPDATE_HAT) {
                buf.write_bool(entry.show_hat);
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let actions = PlayerInfoActions(buf.read_u8()?);
        let entries = read_bounded_vec(buf, MAX_PLAYER_INFO_ENTRIES, 16, |buf| {
            let profile_id = buf.read_uuid()?;
            let mut name = String::new();
            let mut properties = Vec::new();
            let mut listed = false;
            let mut latency = 0;
            let mut game_mode = 0;
            let mut list_order = 0;
            let mut show_hat = false;
            if actions.contains(PlayerInfoActions::ADD_PLAYER) {
                name = buf.read_string(16)?;
                properties = read_bounded_vec(buf, MAX_GAME_PROFILE_PROPERTIES, 3, |buf| {
                    let property_name = buf.read_string(MAX_GAME_PROFILE_PROPERTY_NAME_LEN)?;
                    let value = buf.read_string(MAX_GAME_PROFILE_PROPERTY_VALUE_LEN)?;
                    let signature = if buf.read_bool()? {
                        Some(buf.read_string(MAX_GAME_PROFILE_PROPERTY_SIGNATURE_LEN)?)
                    } else {
                        None
                    };
                    Ok(GameProfileProperty {
                        name: property_name,
                        value,
                        signature,
                    })
                })?;
            }
            if actions.contains(PlayerInfoActions::INITIALIZE_CHAT) {
                return Err(CodecError::NotSupported("player chat session data"));
            }
            if actions.contains(PlayerInfoActions::UPDATE_GAME_MODE) {
                game_mode = buf.read_varint()?;
            }
            if actions.contains(PlayerInfoActions::UPDATE_LISTED) {
                listed = buf.read_bool()?;
            }
            if actions.contains(PlayerInfoActions::UPDATE_LATENCY) {
                latency = buf.read_varint()?;
            }
            if actions.contains(PlayerInfoActions::UPDATE_DISPLAY_NAME) {
                let has_display_name = buf.read_bool()?;
                if has_display_name {
                    return Err(CodecError::NotSupported("player display name component"));
                }
            }
            if actions.contains(PlayerInfoActions::UPDATE_LIST_ORDER) {
                list_order = buf.read_varint()?;
            }
            if actions.contains(PlayerInfoActions::UPDATE_HAT) {
                show_hat = buf.read_bool()?;
            }
            Ok(PlayerInfoEntry {
                profile_id,
                name,
                properties,
                listed,
                latency,
                game_mode,
                list_order,
                show_hat,
            })
        })?;
        Ok(Self { actions, entries })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfoRemove {
    pub profile_ids: Vec<Uuid>,
}

impl Packet for PlayerInfoRemove {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_PLAYER_INFO_REMOVE
    // sits at game-CB index 69 = wire id 0x45. Body is a VarInt-counted UUID list.
    const ID: i32 = 0x45;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_count(buf, self.profile_ids.len())?;
        for id in &self.profile_ids {
            buf.write_uuid(*id);
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let profile_ids = read_bounded_vec(buf, MAX_PLAYER_INFO_ENTRIES, 16, ReadMc::read_uuid)?;
        Ok(Self { profile_ids })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AddEntity {
    pub entity_id: i32,
    pub uuid: Uuid,
    pub entity_type_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub movement: EntityVec3,
    pub pitch: f32,
    pub yaw: f32,
    pub head_yaw: f32,
    pub data: i32,
}

impl Packet for AddEntity {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_ADD_ENTITY is
    // game-CB index 1 = wire id 0x01. `javap` shows VarInt id, UUID,
    // registry entity type id, xyz doubles, `Vec3.LP_STREAM_CODEC`, packed
    // x/y/head rotations, then VarInt data.
    const ID: i32 = 0x01;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_uuid(self.uuid);
        buf.write_varint(self.entity_type_id);
        buf.write_f64(self.x);
        buf.write_f64(self.y);
        buf.write_f64(self.z);
        write_lp_vec3(buf, self.movement);
        buf.write_u8(pack_degrees(self.pitch));
        buf.write_u8(pack_degrees(self.yaw));
        buf.write_u8(pack_degrees(self.head_yaw));
        buf.write_varint(self.data);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            uuid: buf.read_uuid()?,
            entity_type_id: buf.read_varint()?,
            x: buf.read_f64()?,
            y: buf.read_f64()?,
            z: buf.read_f64()?,
            movement: read_lp_vec3(buf)?,
            pitch: unpack_degrees(buf.read_u8()?),
            yaw: unpack_degrees(buf.read_u8()?),
            head_yaw: unpack_degrees(buf.read_u8()?),
            data: buf.read_varint()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityAnimationAction {
    SwingMainHand,
    WakeUp,
    SwingOffHand,
    CriticalHit,
    MagicCriticalHit,
    Raw(u8),
}

impl EntityAnimationAction {
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::SwingMainHand => 0,
            Self::WakeUp => 2,
            Self::SwingOffHand => 3,
            Self::CriticalHit => 4,
            Self::MagicCriticalHit => 5,
            Self::Raw(value) => value,
        }
    }

    #[must_use]
    pub const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::SwingMainHand,
            2 => Self::WakeUp,
            3 => Self::SwingOffHand,
            4 => Self::CriticalHit,
            5 => Self::MagicCriticalHit,
            other => Self::Raw(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityAnimation {
    pub entity_id: i32,
    pub action: EntityAnimationAction,
}

impl Packet for EntityAnimation {
    // Verified via inner `.analysis/server.jar` 26.1.2 server jar:
    // CLIENTBOUND_ANIMATE is game-CB index 2 = wire id 0x02.
    // `javap -p -c ClientboundAnimatePacket` shows VarInt entity id
    // followed by one unsigned byte action.
    const ID: i32 = 0x02;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_u8(self.action.as_u8());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            action: EntityAnimationAction::from_u8(buf.read_u8()?),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityEvent {
    pub entity_id: i32,
    pub event_id: i8,
}

impl Packet for EntityEvent {
    // Verified via inner `.analysis/server.jar` 26.1.2 server jar:
    // CLIENTBOUND_ENTITY_EVENT is game-CB index 34 = wire id 0x22.
    // `javap -p -c ClientboundEntityEventPacket` shows i32 entity id
    // followed by one signed byte event id.
    const ID: i32 = 0x22;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i32(self.entity_id);
        buf.write_i8(self.event_id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_i32()?,
            event_id: buf.read_i8()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveEntityPos {
    pub entity_id: i32,
    pub delta_x: i16,
    pub delta_y: i16,
    pub delta_z: i16,
    pub on_ground: bool,
}

impl MoveEntityPos {
    #[must_use]
    pub fn delta_to_short(delta: f64) -> i16 {
        move_entity_delta_to_short(delta)
    }
}

impl Packet for MoveEntityPos {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_MOVE_ENTITY_POS
    // immediately precedes CLIENTBOUND_MOVE_ENTITY_POS_ROT, so its game-CB
    // index is 53 = wire id 0x35. `ClientboundMoveEntityPacket$Pos` carries
    // VarInt id, three i16 relative deltas, then onGround bool.
    const ID: i32 = 0x35;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_i16(self.delta_x);
        buf.write_i16(self.delta_y);
        buf.write_i16(self.delta_z);
        buf.write_bool(self.on_ground);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            delta_x: buf.read_i16()?,
            delta_y: buf.read_i16()?,
            delta_z: buf.read_i16()?,
            on_ground: buf.read_bool()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveEntityPosRot {
    pub entity_id: i32,
    pub delta_x: i16,
    pub delta_y: i16,
    pub delta_z: i16,
    pub yaw: u8,
    pub pitch: u8,
    pub on_ground: bool,
}

impl MoveEntityPosRot {
    #[must_use]
    pub fn delta_to_short(delta: f64) -> i16 {
        move_entity_delta_to_short(delta)
    }

    #[must_use]
    pub fn pack_degrees(degrees: f32) -> u8 {
        pack_degrees(degrees)
    }
}

impl Packet for MoveEntityPosRot {
    // Verified via inner `.analysis/server.jar` 26.1.2 server jar:
    // CLIENTBOUND_MOVE_ENTITY_POS_ROT is game-CB index 54 = wire id 0x36.
    // `javap -p -c ClientboundMoveEntityPacket$PosRot` shows VarInt id,
    // three i16 relative deltas, packed yRot byte, packed xRot byte,
    // then onGround bool.
    const ID: i32 = 0x36;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_i16(self.delta_x);
        buf.write_i16(self.delta_y);
        buf.write_i16(self.delta_z);
        buf.write_u8(self.yaw);
        buf.write_u8(self.pitch);
        buf.write_bool(self.on_ground);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            delta_x: buf.read_i16()?,
            delta_y: buf.read_i16()?,
            delta_z: buf.read_i16()?,
            yaw: buf.read_u8()?,
            pitch: buf.read_u8()?,
            on_ground: buf.read_bool()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveEntityRot {
    pub entity_id: i32,
    pub yaw: u8,
    pub pitch: u8,
    pub on_ground: bool,
}

impl Packet for MoveEntityRot {
    // `.analysis/protocol-dump.txt`: MOVE_ENTITY_ROT follows the inserted
    // MOVE_MINECART_ALONG_TRACK packet at game-CB index 56 (wire id 0x38).
    // 26.1.2 `ClientboundMoveEntityPacket$Rot` javap shows VarInt id, packed
    // y/x rotation bytes, then onGround.
    const ID: i32 = 0x38;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_u8(self.yaw);
        buf.write_u8(self.pitch);
        buf.write_bool(self.on_ground);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            yaw: buf.read_u8()?,
            pitch: buf.read_u8()?,
            on_ground: buf.read_bool()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityPositionSync {
    pub entity_id: i32,
    pub values: PositionMoveRotation,
    pub on_ground: bool,
}

impl Packet for EntityPositionSync {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_ENTITY_POSITION_SYNC
    // is game-CB index 35 = wire id 0x23. Body is VarInt id,
    // `PositionMoveRotation.STREAM_CODEC`, then onGround bool.
    const ID: i32 = 0x23;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        self.values.encode(buf);
        buf.write_bool(self.on_ground);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            values: PositionMoveRotation::decode(buf)?,
            on_ground: buf.read_bool()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SetEntityMotion {
    pub entity_id: i32,
    pub movement: EntityVec3,
}

impl Packet for SetEntityMotion {
    // Verified via inner `.analysis/server.jar` 26.1.2 server jar:
    // CLIENTBOUND_SET_ENTITY_MOTION is game-CB index 101 = wire id 0x65.
    // `javap -p -c ClientboundSetEntityMotionPacket` shows VarInt id plus
    // `Vec3.LP_STREAM_CODEC` movement.
    const ID: i32 = 0x65;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        write_lp_vec3(buf, self.movement);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            movement: read_lp_vec3(buf)?,
        })
    }
}

/// A non-negative `Block.BLOCK_STATE_REGISTRY` wire id.
///
/// This type distinguishes block-state ids from other registry ids. It does
/// not claim that the id exists in a particular runtime registry; callers
/// must resolve that before constructing packet data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockStateId(i32);

impl BlockStateId {
    pub fn new(raw: u32) -> Result<Self, CodecError> {
        let raw = i32::try_from(raw).map_err(|_| {
            CodecError::NotSupported("block-state registry id exceeds VarInt range")
        })?;
        Ok(Self(raw))
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0 as u32
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let raw = buf.read_varint()?;
        if raw < 0 {
            return Err(CodecError::NotSupported("negative block-state registry id"));
        }
        Ok(Self(raw))
    }
}

/// Three big-endian floats used by entity-data serializer id 9.
#[derive(Debug, Clone, Copy)]
pub struct EntityRotations {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl PartialEq for EntityRotations {
    fn eq(&self, other: &Self) -> bool {
        java_float_equals(self.x, other.x)
            && java_float_equals(self.y, other.y)
            && java_float_equals(self.z, other.z)
    }
}

impl Eq for EntityRotations {}

/// The six direction ids established by `Direction.STREAM_CODEC`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityDirection {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl EntityDirection {
    const fn wire_id(self) -> i32 {
        match self {
            Self::Down => 0,
            Self::Up => 1,
            Self::North => 2,
            Self::South => 3,
            Self::West => 4,
            Self::East => 5,
        }
    }

    fn from_wire(value: i32) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::Down,
            1 => Self::Up,
            2 => Self::North,
            3 => Self::South,
            4 => Self::West,
            5 => Self::East,
            _ => return Err(CodecError::NotSupported("unknown entity direction")),
        })
    }
}

/// Particle options emitted by vanilla status effects in 26.1.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectParticle {
    EntityEffect { argb: i32 },
    TrialOmen,
    RaidOmen,
}

impl EffectParticle {
    fn encode<B: BufMut>(self, buf: &mut B) {
        match self {
            Self::EntityEffect { argb } => {
                buf.write_varint(PARTICLE_TYPE_ENTITY_EFFECT);
                buf.write_i32(argb);
            }
            Self::TrialOmen => buf.write_varint(PARTICLE_TYPE_TRIAL_OMEN),
            Self::RaidOmen => buf.write_varint(PARTICLE_TYPE_RAID_OMEN),
        }
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        match buf.read_varint()? {
            PARTICLE_TYPE_ENTITY_EFFECT => Ok(Self::EntityEffect {
                argb: buf.read_i32()?,
            }),
            PARTICLE_TYPE_TRIAL_OMEN => Ok(Self::TrialOmen),
            PARTICLE_TYPE_RAID_OMEN => Ok(Self::RaidOmen),
            _ => Err(CodecError::NotSupported(
                "unsupported effect metadata particle",
            )),
        }
    }
}

/// Entity-data serializers implemented for 26.1.2.
///
/// Supported serializer ids are 0-4, 6-20, and 42. Particle serializers
/// support the options used by built-in effects; other particle types and
/// variant, global-position, vector, quaternion, and resolvable-profile
/// serializers are rejected explicitly.
#[derive(Debug, Clone)]
pub enum EntityDataValue {
    /// CopperGolem enum serializers, javap EntityDataSerializers 26.1.2:
    /// state = 37, weathering = 38; both use VarInt ordinal codecs.
    CopperGolemState {
        index: u8,
        value: u8,
    },
    WeatheringCopper {
        index: u8,
        value: u8,
    },
    Byte {
        index: u8,
        value: i8,
    },
    Int {
        index: u8,
        value: i32,
    },
    Long {
        index: u8,
        value: i64,
    },
    Float {
        index: u8,
        value: f32,
    },
    String {
        index: u8,
        value: String,
    },
    OptionalComponent {
        index: u8,
        value: Option<String>,
    },
    ItemStack {
        index: u8,
        stack: ItemStack,
    },
    Boolean {
        index: u8,
        value: bool,
    },
    Rotations {
        index: u8,
        value: EntityRotations,
    },
    /// Packed `BlockPos::asLong()` value.
    BlockPosition {
        index: u8,
        value: i64,
    },
    OptionalBlockPosition {
        index: u8,
        value: Option<i64>,
    },
    Direction {
        index: u8,
        value: EntityDirection,
    },
    OptionalLivingEntityReference {
        index: u8,
        value: Option<Uuid>,
    },
    BlockState {
        index: u8,
        value: BlockStateId,
    },
    OptionalBlockState {
        index: u8,
        value: Option<BlockStateId>,
    },
    VillagerData {
        index: u8,
        villager_type: i32,
        profession: i32,
        level: i32,
    },
    OptionalUnsignedInt {
        index: u8,
        value: Option<u32>,
    },
    Pose {
        index: u8,
        pose: EntityPose,
    },
    HumanoidArm {
        index: u8,
        value: MainHand,
    },
    /// Entity-effect particle metadata, such as the `AreaEffectCloud` tint.
    EntityEffectParticle {
        index: u8,
        argb: i32,
    },
    /// LivingEntity.DATA_EFFECT_PARTICLES: VarInt count + particle stream codecs.
    EffectParticles {
        index: u8,
        particles: Vec<EffectParticle>,
    },
}

impl PartialEq for EntityDataValue {
    fn eq(&self, other: &Self) -> bool {
        if self.index() != other.index() || self.serializer_id() != other.serializer_id() {
            return false;
        }

        match (self, other) {
            (Self::Byte { value: left, .. }, Self::Byte { value: right, .. }) => left == right,
            (
                Self::CopperGolemState { value: left, .. },
                Self::CopperGolemState { value: right, .. },
            )
            | (
                Self::WeatheringCopper { value: left, .. },
                Self::WeatheringCopper { value: right, .. },
            ) => left == right,
            (Self::Int { value: left, .. }, Self::Int { value: right, .. }) => left == right,
            (Self::Long { value: left, .. }, Self::Long { value: right, .. }) => left == right,
            (Self::Float { value: left, .. }, Self::Float { value: right, .. }) => {
                java_float_equals(*left, *right)
            }
            (Self::String { value: left, .. }, Self::String { value: right, .. }) => left == right,
            (
                Self::OptionalComponent { value: left, .. },
                Self::OptionalComponent { value: right, .. },
            ) => left == right,
            (Self::ItemStack { stack: left, .. }, Self::ItemStack { stack: right, .. }) => {
                left == right
            }
            (Self::Boolean { value: left, .. }, Self::Boolean { value: right, .. }) => {
                left == right
            }
            (Self::Rotations { value: left, .. }, Self::Rotations { value: right, .. }) => {
                left == right
            }
            (Self::BlockPosition { value: left, .. }, Self::BlockPosition { value: right, .. }) => {
                left == right
            }
            (
                Self::OptionalBlockPosition { value: left, .. },
                Self::OptionalBlockPosition { value: right, .. },
            ) => left == right,
            (Self::Direction { value: left, .. }, Self::Direction { value: right, .. }) => {
                left == right
            }
            (
                Self::OptionalLivingEntityReference { value: left, .. },
                Self::OptionalLivingEntityReference { value: right, .. },
            ) => left == right,
            (Self::BlockState { value: left, .. }, Self::BlockState { value: right, .. }) => {
                left == right
            }
            (
                Self::OptionalBlockState { value: left, .. },
                Self::OptionalBlockState { value: right, .. },
            ) => left == right,
            (
                Self::VillagerData {
                    villager_type: left_type,
                    profession: left_profession,
                    level: left_level,
                    ..
                },
                Self::VillagerData {
                    villager_type: right_type,
                    profession: right_profession,
                    level: right_level,
                    ..
                },
            ) => {
                left_type == right_type
                    && left_profession == right_profession
                    && left_level == right_level
            }
            (
                Self::OptionalUnsignedInt { value: left, .. },
                Self::OptionalUnsignedInt { value: right, .. },
            ) => left == right,
            (Self::Pose { pose: left, .. }, Self::Pose { pose: right, .. }) => left == right,
            (Self::HumanoidArm { value: left, .. }, Self::HumanoidArm { value: right, .. }) => {
                left == right
            }
            (
                Self::EntityEffectParticle { argb: left, .. },
                Self::EntityEffectParticle { argb: right, .. },
            ) => left == right,
            (
                Self::EffectParticles {
                    particles: left, ..
                },
                Self::EffectParticles {
                    particles: right, ..
                },
            ) => left == right,
            _ => false,
        }
    }
}

impl Eq for EntityDataValue {}

impl EntityDataValue {
    #[must_use]
    pub const fn index(&self) -> u8 {
        match self {
            Self::Byte { index, .. }
            | Self::CopperGolemState { index, .. }
            | Self::WeatheringCopper { index, .. }
            | Self::Int { index, .. }
            | Self::Long { index, .. }
            | Self::Float { index, .. }
            | Self::String { index, .. }
            | Self::OptionalComponent { index, .. }
            | Self::ItemStack { index, .. }
            | Self::Boolean { index, .. }
            | Self::Rotations { index, .. }
            | Self::BlockPosition { index, .. }
            | Self::OptionalBlockPosition { index, .. }
            | Self::Direction { index, .. }
            | Self::OptionalLivingEntityReference { index, .. }
            | Self::BlockState { index, .. }
            | Self::OptionalBlockState { index, .. }
            | Self::VillagerData { index, .. }
            | Self::OptionalUnsignedInt { index, .. }
            | Self::Pose { index, .. }
            | Self::HumanoidArm { index, .. }
            | Self::EntityEffectParticle { index, .. }
            | Self::EffectParticles { index, .. } => *index,
        }
    }

    #[must_use]
    pub const fn serializer_id(&self) -> i32 {
        match self {
            Self::Byte { .. } => ENTITY_DATA_BYTE_SERIALIZER_ID,
            Self::CopperGolemState { .. } => 37,
            Self::WeatheringCopper { .. } => 38,
            Self::Int { .. } => ENTITY_DATA_INT_SERIALIZER_ID,
            Self::Long { .. } => ENTITY_DATA_LONG_SERIALIZER_ID,
            Self::Float { .. } => ENTITY_DATA_FLOAT_SERIALIZER_ID,
            Self::String { .. } => ENTITY_DATA_STRING_SERIALIZER_ID,
            Self::OptionalComponent { .. } => 6,
            Self::ItemStack { .. } => ENTITY_DATA_ITEM_STACK_SERIALIZER_ID,
            Self::Boolean { .. } => ENTITY_DATA_BOOLEAN_SERIALIZER_ID,
            Self::Rotations { .. } => ENTITY_DATA_ROTATIONS_SERIALIZER_ID,
            Self::BlockPosition { .. } => ENTITY_DATA_BLOCK_POSITION_SERIALIZER_ID,
            Self::OptionalBlockPosition { .. } => ENTITY_DATA_OPTIONAL_BLOCK_POSITION_SERIALIZER_ID,
            Self::Direction { .. } => ENTITY_DATA_DIRECTION_SERIALIZER_ID,
            Self::OptionalLivingEntityReference { .. } => {
                ENTITY_DATA_OPTIONAL_LIVING_ENTITY_REFERENCE_SERIALIZER_ID
            }
            Self::BlockState { .. } => ENTITY_DATA_BLOCK_STATE_SERIALIZER_ID,
            Self::OptionalBlockState { .. } => ENTITY_DATA_OPTIONAL_BLOCK_STATE_SERIALIZER_ID,
            Self::VillagerData { .. } => ENTITY_DATA_VILLAGER_DATA_SERIALIZER_ID,
            Self::OptionalUnsignedInt { .. } => ENTITY_DATA_OPTIONAL_UNSIGNED_INT_SERIALIZER_ID,
            Self::Pose { .. } => ENTITY_DATA_POSE_SERIALIZER_ID,
            Self::HumanoidArm { .. } => ENTITY_DATA_HUMANOID_ARM_SERIALIZER_ID,
            Self::EntityEffectParticle { .. } => ENTITY_DATA_PARTICLE_SERIALIZER_ID,
            Self::EffectParticles { .. } => ENTITY_DATA_PARTICLES_SERIALIZER_ID,
        }
    }

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_u8(self.index());
        buf.write_varint(self.serializer_id());
        match self {
            Self::Byte { value, .. } => buf.write_i8(*value),
            Self::CopperGolemState { value, .. } | Self::WeatheringCopper { value, .. } => {
                buf.write_varint(i32::from(*value))
            }
            Self::Int { value, .. } => buf.write_varint(*value),
            Self::Long { value, .. } => buf.write_varlong(*value),
            Self::Float { value, .. } => buf.write_f32(*value),
            Self::String { value, .. } => {
                buf.write_string(value, DEFAULT_MAX_STRING_LEN)?;
            }
            Self::OptionalComponent { value, .. } => {
                buf.write_bool(value.is_some());
                if let Some(text) = value {
                    write_literal_text_component(buf, text)?;
                }
            }
            Self::ItemStack { stack, .. } => stack.encode(buf)?,
            Self::Boolean { value, .. } => buf.write_bool(*value),
            Self::Rotations { value, .. } => {
                buf.write_f32(value.x);
                buf.write_f32(value.y);
                buf.write_f32(value.z);
            }
            Self::BlockPosition { value, .. } => buf.write_i64(*value),
            Self::OptionalBlockPosition { value, .. } => {
                buf.write_bool(value.is_some());
                if let Some(value) = value {
                    buf.write_i64(*value);
                }
            }
            Self::Direction { value, .. } => buf.write_varint(value.wire_id()),
            Self::OptionalLivingEntityReference { value, .. } => {
                buf.write_bool(value.is_some());
                if let Some(value) = value {
                    buf.write_uuid(*value);
                }
            }
            Self::BlockState { value, .. } => buf.write_varint(value.0),
            Self::OptionalBlockState { value, .. } => match value {
                None => buf.write_varint(0),
                Some(value) if value.0 == 0 => {
                    return Err(CodecError::NotSupported(
                        "optional block-state id zero means absent",
                    ));
                }
                Some(value) => buf.write_varint(value.0),
            },
            Self::VillagerData {
                villager_type,
                profession,
                level,
                ..
            } => {
                buf.write_varint(*villager_type);
                buf.write_varint(*profession);
                buf.write_varint(*level);
            }
            Self::OptionalUnsignedInt { value, .. } => match value {
                None => buf.write_varint(0),
                Some(value) => {
                    let wire = value
                        .checked_add(1)
                        .and_then(|value| i32::try_from(value).ok())
                        .ok_or(CodecError::NotSupported(
                            "optional unsigned integer exceeds VarInt range",
                        ))?;
                    buf.write_varint(wire);
                }
            },
            Self::Pose { pose, .. } => buf.write_varint(*pose as i32),
            Self::HumanoidArm { value, .. } => buf.write_varint(match value {
                MainHand::Left => 0,
                MainHand::Right => 1,
            }),
            Self::EntityEffectParticle { argb, .. } => {
                buf.write_varint(PARTICLE_TYPE_ENTITY_EFFECT);
                buf.write_i32(*argb);
            }
            Self::EffectParticles { particles, .. } => {
                write_count(buf, particles.len())?;
                for particle in particles {
                    particle.encode(buf);
                }
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(index: u8, serializer_id: i32, buf: &mut B) -> Result<Self, CodecError> {
        Ok(match serializer_id {
            ENTITY_DATA_BYTE_SERIALIZER_ID => Self::Byte {
                index,
                value: buf.read_i8()?,
            },
            ENTITY_DATA_INT_SERIALIZER_ID => Self::Int {
                index,
                value: buf.read_varint()?,
            },
            ENTITY_DATA_LONG_SERIALIZER_ID => Self::Long {
                index,
                value: buf.read_varlong()?,
            },
            ENTITY_DATA_FLOAT_SERIALIZER_ID => Self::Float {
                index,
                value: buf.read_f32()?,
            },
            ENTITY_DATA_STRING_SERIALIZER_ID => Self::String {
                index,
                value: buf.read_string(DEFAULT_MAX_STRING_LEN)?,
            },
            6 => Self::OptionalComponent {
                index,
                value: if buf.read_bool()? {
                    Some(read_literal_text_component(buf)?)
                } else {
                    None
                },
            },
            ENTITY_DATA_ITEM_STACK_SERIALIZER_ID => Self::ItemStack {
                index,
                stack: ItemStack::decode(buf)?,
            },
            ENTITY_DATA_BOOLEAN_SERIALIZER_ID => Self::Boolean {
                index,
                value: buf.read_bool()?,
            },
            ENTITY_DATA_ROTATIONS_SERIALIZER_ID => Self::Rotations {
                index,
                value: EntityRotations {
                    x: buf.read_f32()?,
                    y: buf.read_f32()?,
                    z: buf.read_f32()?,
                },
            },
            ENTITY_DATA_BLOCK_POSITION_SERIALIZER_ID => Self::BlockPosition {
                index,
                value: buf.read_i64()?,
            },
            ENTITY_DATA_OPTIONAL_BLOCK_POSITION_SERIALIZER_ID => Self::OptionalBlockPosition {
                index,
                value: if buf.read_bool()? {
                    Some(buf.read_i64()?)
                } else {
                    None
                },
            },
            ENTITY_DATA_DIRECTION_SERIALIZER_ID => Self::Direction {
                index,
                value: EntityDirection::from_wire(buf.read_varint()?)?,
            },
            ENTITY_DATA_OPTIONAL_LIVING_ENTITY_REFERENCE_SERIALIZER_ID => {
                Self::OptionalLivingEntityReference {
                    index,
                    value: if buf.read_bool()? {
                        Some(buf.read_uuid()?)
                    } else {
                        None
                    },
                }
            }
            ENTITY_DATA_BLOCK_STATE_SERIALIZER_ID => Self::BlockState {
                index,
                value: BlockStateId::decode(buf)?,
            },
            ENTITY_DATA_OPTIONAL_BLOCK_STATE_SERIALIZER_ID => {
                let raw = buf.read_varint()?;
                if raw < 0 {
                    return Err(CodecError::NotSupported("negative block-state registry id"));
                }
                Self::OptionalBlockState {
                    index,
                    value: (raw != 0).then_some(BlockStateId(raw)),
                }
            }
            ENTITY_DATA_VILLAGER_DATA_SERIALIZER_ID => Self::VillagerData {
                index,
                villager_type: buf.read_varint()?,
                profession: buf.read_varint()?,
                level: buf.read_varint()?,
            },
            ENTITY_DATA_OPTIONAL_UNSIGNED_INT_SERIALIZER_ID => {
                let raw = buf.read_varint()?;
                if raw < 0 {
                    return Err(CodecError::NotSupported(
                        "negative optional unsigned integer",
                    ));
                }
                Self::OptionalUnsignedInt {
                    index,
                    value: (raw != 0).then_some((raw - 1) as u32),
                }
            }
            ENTITY_DATA_POSE_SERIALIZER_ID => Self::Pose {
                index,
                pose: EntityPose::from_wire(buf.read_varint()?)?,
            },
            ENTITY_DATA_HUMANOID_ARM_SERIALIZER_ID => Self::HumanoidArm {
                index,
                value: match buf.read_varint()? {
                    0 => MainHand::Left,
                    1 => MainHand::Right,
                    _ => return Err(CodecError::NotSupported("unknown humanoid arm")),
                },
            },
            ENTITY_DATA_PARTICLE_SERIALIZER_ID => {
                if buf.read_varint()? != PARTICLE_TYPE_ENTITY_EFFECT {
                    return Err(CodecError::NotSupported("unsupported entity data particle"));
                }
                Self::EntityEffectParticle {
                    index,
                    argb: buf.read_i32()?,
                }
            }
            ENTITY_DATA_PARTICLES_SERIALIZER_ID => Self::EffectParticles {
                index,
                particles: read_bounded_vec(
                    buf,
                    MAX_ENTITY_EFFECT_PARTICLES,
                    1,
                    EffectParticle::decode,
                )?,
            },
            37 | 38 => {
                let value = buf.read_varint()?;
                let maximum = if serializer_id == 37 { 4 } else { 3 };
                if !(0..=maximum).contains(&value) {
                    return Err(CodecError::NotSupported(
                        "invalid copper golem metadata ordinal",
                    ));
                }
                if serializer_id == 37 {
                    Self::CopperGolemState {
                        index,
                        value: value as u8,
                    }
                } else {
                    Self::WeatheringCopper {
                        index,
                        value: value as u8,
                    }
                }
            }
            _ => {
                return Err(CodecError::NotSupported(
                    "entity data serializer is not implemented",
                ));
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityPose {
    Standing = 0,
    FallFlying = 1,
    Sleeping = 2,
    Swimming = 3,
    SpinAttack = 4,
    Crouching = 5,
    LongJumping = 6,
    Dying = 7,
    Croaking = 8,
    UsingTongue = 9,
    Sitting = 10,
    Roaring = 11,
    Sniffing = 12,
    Emerging = 13,
    Digging = 14,
    Sliding = 15,
    Shooting = 16,
    Inhaling = 17,
}

impl EntityPose {
    fn from_wire(value: i32) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::Standing,
            1 => Self::FallFlying,
            2 => Self::Sleeping,
            3 => Self::Swimming,
            4 => Self::SpinAttack,
            5 => Self::Crouching,
            6 => Self::LongJumping,
            7 => Self::Dying,
            8 => Self::Croaking,
            9 => Self::UsingTongue,
            10 => Self::Sitting,
            11 => Self::Roaring,
            12 => Self::Sniffing,
            13 => Self::Emerging,
            14 => Self::Digging,
            15 => Self::Sliding,
            16 => Self::Shooting,
            17 => Self::Inhaling,
            _ => return Err(CodecError::NotSupported("unknown entity pose")),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundSetEntityData {
    pub entity_id: i32,
    pub values: Vec<EntityDataValue>,
}

impl Packet for ClientboundSetEntityData {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_SET_ENTITY_DATA is
    // game-CB index 99 = wire id 0x63. `javap -p -c` shows VarInt entity id,
    // repeated DataValue entries, then EOF marker byte 255. DataValue writes one
    // unsigned-byte metadata index, VarInt serializer id, then serializer payload.
    const ID: i32 = 0x63;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.values.len() > MAX_ENTITY_DATA_VALUES {
            return Err(CodecError::StringTooLong {
                len: self.values.len(),
                max: MAX_ENTITY_DATA_VALUES,
            });
        }
        let mut seen_indices = [false; 0xFF];
        for value in &self.values {
            if let EntityDataValue::EffectParticles { particles, .. } = value
                && particles.len() > MAX_ENTITY_EFFECT_PARTICLES
            {
                return Err(CodecError::StringTooLong {
                    len: particles.len(),
                    max: MAX_ENTITY_EFFECT_PARTICLES,
                });
            }
            let index = value.index();
            if index == 0xFF {
                return Err(CodecError::NotSupported(
                    "entity data index 255 is reserved",
                ));
            }
            if seen_indices[index as usize] {
                return Err(CodecError::NotSupported("duplicate entity data index"));
            }
            seen_indices[index as usize] = true;
        }

        buf.write_varint(self.entity_id);
        for value in &self.values {
            value.encode(buf)?;
        }
        buf.write_u8(0xFF);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let entity_id = buf.read_varint()?;
        let mut values = Vec::new();
        let mut seen_indices = [false; 0xFF];
        loop {
            let index = buf.read_u8()?;
            if index == 0xFF {
                break;
            }
            if values.len() >= MAX_ENTITY_DATA_VALUES {
                return Err(CodecError::StringTooLong {
                    len: values.len() + 1,
                    max: MAX_ENTITY_DATA_VALUES,
                });
            }
            if seen_indices[index as usize] {
                return Err(CodecError::NotSupported("duplicate entity data index"));
            }
            seen_indices[index as usize] = true;
            let serializer_id = buf.read_varint()?;
            values.push(EntityDataValue::decode(index, serializer_id, buf)?);
        }
        Ok(Self { entity_id, values })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundTakeItemEntity {
    pub item_entity_id: i32,
    pub player_entity_id: i32,
    pub amount: i32,
}

impl Packet for ClientboundTakeItemEntity {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_TAKE_ITEM_ENTITY is
    // game-CB index 124 = wire id 0x7C. `javap -p -c` shows three VarInts:
    // item entity id, player entity id, and picked-up amount.
    const ID: i32 = 0x7C;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.item_entity_id);
        buf.write_varint(self.player_entity_id);
        buf.write_varint(self.amount);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            item_entity_id: buf.read_varint()?,
            player_entity_id: buf.read_varint()?,
            amount: buf.read_varint()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveEntities {
    pub entity_ids: Vec<i32>,
}

impl Packet for RemoveEntities {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_REMOVE_ENTITIES
    // is game-CB index 77 = wire id 0x4D. Body is FriendlyByteBuf.writeIntIdList.
    const ID: i32 = 0x4D;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_count(buf, self.entity_ids.len())?;
        for id in &self.entity_ids {
            buf.write_varint(*id);
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        // `readIntIdList` has no semantic cap; the frame bounds it and
        // `read_bounded_vec` rejects counts the remaining bytes cannot hold.
        let entity_ids = read_bounded_vec(buf, usize::MAX, 1, ReadMc::read_varint)?;
        Ok(Self { entity_ids })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundSetPassengers {
    pub vehicle_id: i32,
    pub passenger_ids: Vec<i32>,
}

impl Packet for ClientboundSetPassengers {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_SET_PASSENGERS
    // is game-CB index 107 = wire id 0x6B. Body is a vehicle VarInt followed
    // by a VarInt-length-prefixed array of passenger entity VarInts.
    const ID: i32 = 0x6B;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.vehicle_id);
        write_bounded_count(buf, self.passenger_ids.len(), MAX_ENTITY_ID_LIST_LEN)?;
        for passenger_id in &self.passenger_ids {
            buf.write_varint(*passenger_id);
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let vehicle_id = buf.read_varint()?;
        let passenger_ids = read_bounded_vec(buf, MAX_ENTITY_ID_LIST_LEN, 1, ReadMc::read_varint)?;
        Ok(Self {
            vehicle_id,
            passenger_ids,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RotateHead {
    pub entity_id: i32,
    pub head_yaw: f32,
}

impl Packet for RotateHead {
    // Verified via `.analysis/protocol-dump.txt`: CLIENTBOUND_ROTATE_HEAD is
    // game-CB index 83 = wire id 0x53. Body is VarInt entity id and packed yaw.
    const ID: i32 = 0x53;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_u8(pack_degrees(self.head_yaw));
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            head_yaw: unpack_degrees(buf.read_u8()?),
        })
    }
}

#[cfg(test)]
#[path = "entity_copper_tests.rs"]
mod copper_tests;
