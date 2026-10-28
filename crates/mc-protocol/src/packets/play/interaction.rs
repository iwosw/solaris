//! Play interaction packets: movement, actions, use-item, attack, resource packs.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundResourcePack {
    pub status: ResourcePackStatus,
}

impl Packet for ServerboundResourcePack {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_RESOURCE_PACK is
    // serverbound registration index 49, wire id 0x31. Body is local decompiled
    // `ServerboundResourcePackPacket(UUID id, Action action)`.
    const ID: i32 = 0x32;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.status.encode(buf)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            status: ResourcePackStatus::decode(buf)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundClientInformation {
    pub information: ClientInformation,
}

impl Packet for ServerboundClientInformation {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_CLIENT_INFORMATION is
    // serverbound registration index 14, wire id 0x0E. Body delegates to local
    // decompiled `ServerboundClientInformationPacket(ClientInformation)`.
    const ID: i32 = 0x0E;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.information.encode(buf)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            information: ClientInformation::decode(buf)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundCustomPayload {
    pub payload: CustomPayload,
}

impl Packet for ServerboundCustomPayload {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_CUSTOM_PAYLOAD is
    // serverbound registration index 22, wire id 0x16. Body is the common
    // custom-payload codec from local decompiled sources.
    const ID: i32 = 0x16;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.payload.encode_serverbound(buf)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            payload: CustomPayload::decode_serverbound(buf)?,
        })
    }
}

/// `Confirm Teleportation` (SB). Client echoes our
/// `SynchronizePlayerPosition.teleport_id` back to confirm it accepted
/// the snap. 26.3 appends the client's resulting position and rotation
/// (it no longer sends a separate move packet after a teleport).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConfirmTeleportation {
    pub teleport_id: i32,
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
}

impl ConfirmTeleportation {
    /// The confirmation a client sends after applying an absolute
    /// (`relative_flags == 0`) [`SynchronizePlayerPosition`]: it echoes the
    /// resulting position and rotation, exactly as the vanilla 26.3 client.
    #[must_use]
    pub fn accepting(sync: &SynchronizePlayerPosition) -> Self {
        Self {
            teleport_id: sync.teleport_id,
            position: [sync.x, sync.y, sync.z],
            yaw: sync.yaw,
            pitch: sync.pitch,
        }
    }
}

impl Packet for ConfirmTeleportation {
    // 26.3 `ServerboundAcceptTeleportationPacket.STREAM_CODEC`: VarInt id,
    // f64 x/y/z, f32 yRot, f32 xRot. Game-SB index 0.
    const ID: i32 = 0x00;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.teleport_id);
        for coordinate in self.position {
            buf.write_f64(coordinate);
        }
        buf.write_f32(self.yaw);
        buf.write_f32(self.pitch);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let teleport_id = buf.read_varint()?;
        let position = [buf.read_f64()?, buf.read_f64()?, buf.read_f64()?];
        let yaw = buf.read_f32()?;
        let pitch = buf.read_f32()?;
        if !position.iter().all(|value| value.is_finite()) || !yaw.is_finite() || !pitch.is_finite()
        {
            return Err(CodecError::NotSupported("non-finite teleport confirmation"));
        }
        Ok(Self {
            teleport_id,
            position,
            yaw,
            pitch,
        })
    }
}

/// Serverbound `KeepAlive (Play)`. Echo of the value we sent in
/// [`ClientboundKeepAlive`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundKeepAlive {
    pub id: i64,
}

impl Packet for ServerboundKeepAlive {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // SERVERBOUND_KEEP_ALIVE at game-SB index 28 = wire id 0x1C.
    const ID: i32 = 0x1C;

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

/// `Serverbound Attack` (SB). Verified against the local vanilla 26.1.2 class:
/// `ServerboundAttackPacket(int entityId)` uses `ByteBufCodecs.VAR_INT` and is
/// registered immediately after `SERVERBOUND_ACCEPT_TELEPORTATION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundAttack {
    pub entity_id: i32,
}

impl Packet for ServerboundAttack {
    const ID: i32 = 0x01;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
        })
    }
}

/// `Serverbound Interact` (SB). Verified against the local vanilla 26.1.2 class:
/// `ServerboundInteractPacket(int entityId, InteractionHand hand, Vec3 location,
/// boolean usingSecondaryAction)` uses `Vec3.LP_STREAM_CODEC`. Entity attacks are
/// carried by `ServerboundAttackPacket`, not by this packet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerboundInteract {
    pub entity_id: i32,
    pub hand: InteractionHand,
    pub location: EntityVec3,
    pub using_secondary_action: bool,
}

impl Packet for ServerboundInteract {
    const ID: i32 = 0x1A;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_varint(self.hand as i32);
        write_lp_vec3(buf, self.location);
        buf.write_bool(self.using_secondary_action);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            hand: interaction_hand_from_wire(buf.read_varint()?)?,
            location: read_lp_vec3(buf)?,
            using_secondary_action: buf.read_bool()?,
        })
    }
}

/// `Serverbound Pick Item From Block` (SB, middle click). Local 26.1.2
/// `reports/packets.json`: `minecraft:pick_item_from_block` = 36 (0x24);
/// `javap`: `ServerboundPickItemFromBlockPacket(BlockPos pos, boolean
/// includeData)` via `composite(BlockPos.STREAM_CODEC, ByteBufCodecs.BOOL)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundPickItemFromBlock {
    pub position: i64,
    pub include_data: bool,
}

impl Packet for ServerboundPickItemFromBlock {
    const ID: i32 = 0x24;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(self.position);
        buf.write_bool(self.include_data);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            position: buf.read_i64()?,
            include_data: buf.read_bool()?,
        })
    }
}

/// `Serverbound Pick Item From Entity` (SB, middle click). Local 26.1.2
/// `reports/packets.json`: `minecraft:pick_item_from_entity` = 37 (0x25);
/// `javap`: `ServerboundPickItemFromEntityPacket(int id, boolean includeData)`
/// via `composite(ByteBufCodecs.VAR_INT, ByteBufCodecs.BOOL)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundPickItemFromEntity {
    pub entity_id: i32,
    pub include_data: bool,
}

impl Packet for ServerboundPickItemFromEntity {
    const ID: i32 = 0x25;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_bool(self.include_data);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            include_data: buf.read_bool()?,
        })
    }
}

/// Flags shared by vanilla's four `ServerboundMovePlayerPacket` variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MovePlayerFlags {
    pub on_ground: bool,
    pub horizontal_collision: bool,
}

impl MovePlayerFlags {
    #[must_use]
    pub fn new(on_ground: bool, horizontal_collision: bool) -> Self {
        Self {
            on_ground,
            horizontal_collision,
        }
    }

    fn from_byte(byte: u8) -> Self {
        Self {
            on_ground: byte & 0x01 != 0,
            horizontal_collision: byte & 0x02 != 0,
        }
    }

    fn to_byte(self) -> u8 {
        u8::from(self.on_ground) | (u8::from(self.horizontal_collision) << 1)
    }
}

/// `ServerboundMovePlayerPacket$Pos` (SB). Carries position plus
/// movement flags. Per ADR 0002, verified against javap of vanilla
/// 26.1.2: `SERVERBOUND_MOVE_PLAYER_POS`, stream order
/// `double x, double y, double z, unsigned byte flags`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerboundMovePlayerPos {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub flags: MovePlayerFlags,
}

impl Packet for ServerboundMovePlayerPos {
    // GameProtocols serverbound index 30 = wire id 0x1E.
    const ID: i32 = 0x1E;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_f64(self.x);
        buf.write_f64(self.y);
        buf.write_f64(self.z);
        buf.write_u8(self.flags.to_byte());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            x: buf.read_f64()?,
            y: buf.read_f64()?,
            z: buf.read_f64()?,
            flags: MovePlayerFlags::from_byte(buf.read_u8()?),
        })
    }
}

/// `ServerboundMovePlayerPacket$PosRot` (SB). Carries position,
/// rotation, and movement flags. Per ADR 0002, verified against javap
/// of vanilla 26.1.2: `SERVERBOUND_MOVE_PLAYER_POS_ROT`, stream order
/// `double x, double y, double z, float yRot, float xRot, unsigned byte flags`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerboundMovePlayerPosRot {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub flags: MovePlayerFlags,
}

impl Packet for ServerboundMovePlayerPosRot {
    // GameProtocols serverbound index 31 = wire id 0x1F.
    const ID: i32 = 0x1F;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_f64(self.x);
        buf.write_f64(self.y);
        buf.write_f64(self.z);
        buf.write_f32(self.yaw);
        buf.write_f32(self.pitch);
        buf.write_u8(self.flags.to_byte());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            x: buf.read_f64()?,
            y: buf.read_f64()?,
            z: buf.read_f64()?,
            yaw: buf.read_f32()?,
            pitch: buf.read_f32()?,
            flags: MovePlayerFlags::from_byte(buf.read_u8()?),
        })
    }
}

/// `ServerboundMovePlayerPacket$Rot` (SB). Carries rotation plus
/// movement flags. Per ADR 0002, verified against javap of vanilla
/// 26.1.2: `SERVERBOUND_MOVE_PLAYER_ROT`, stream order
/// `float yRot, float xRot, unsigned byte flags`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerboundMovePlayerRot {
    pub yaw: f32,
    pub pitch: f32,
    pub flags: MovePlayerFlags,
}

impl Packet for ServerboundMovePlayerRot {
    // GameProtocols serverbound index 32 = wire id 0x20.
    const ID: i32 = 0x20;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_f32(self.yaw);
        buf.write_f32(self.pitch);
        buf.write_u8(self.flags.to_byte());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            yaw: buf.read_f32()?,
            pitch: buf.read_f32()?,
            flags: MovePlayerFlags::from_byte(buf.read_u8()?),
        })
    }
}

/// `ServerboundMovePlayerPacket$StatusOnly` (SB). Carries movement
/// flags without position or rotation. Per ADR 0002, verified against
/// javap of vanilla 26.1.2: `SERVERBOUND_MOVE_PLAYER_STATUS_ONLY`,
/// stream order `unsigned byte flags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundMovePlayerStatusOnly {
    pub flags: MovePlayerFlags,
}

impl Packet for ServerboundMovePlayerStatusOnly {
    // GameProtocols serverbound index 33 = wire id 0x21.
    const ID: i32 = 0x21;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_u8(self.flags.to_byte());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            flags: MovePlayerFlags::from_byte(buf.read_u8()?),
        })
    }
}

// ---------------------------------------------------------------------
// M5.b — clientbound edit / ack / relight packets

/// One of the eight actions vanilla packs into
/// `ServerboundPlayerAction`. Enum discriminants match the wire
/// VarInt: the order is the vanilla source's declaration order
/// (`ServerboundPlayerActionPacket$Action`), verified against
/// `javap -p` on the unobfuscated jar.
///
/// The variants follow the 26.3 `ServerboundPlayerActionPacket$Action`
/// ordinal order (`readEnum`), which inserted `CHANGE_DESTROY_DIRECTION` at 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerActionKind {
    StartDestroyBlock = 0,
    ChangeDestroyDirection = 1,
    AbortDestroyBlock = 2,
    StopDestroyBlock = 3,
    DropAllItems = 4,
    DropItem = 5,
    ReleaseUseItem = 6,
    SwapItemWithOffhand = 7,
    Stab = 8,
}

impl PlayerActionKind {
    fn from_wire(v: i32) -> Result<Self, CodecError> {
        Ok(match v {
            0 => Self::StartDestroyBlock,
            1 => Self::ChangeDestroyDirection,
            2 => Self::AbortDestroyBlock,
            3 => Self::StopDestroyBlock,
            4 => Self::DropAllItems,
            5 => Self::DropItem,
            6 => Self::ReleaseUseItem,
            7 => Self::SwapItemWithOffhand,
            8 => Self::Stab,
            other => {
                return Err(CodecError::StringTooLong {
                    len: other as usize,
                    max: 8,
                });
            }
        })
    }
}

/// `Serverbound Player Action` (SB). Carries break-start /
/// break-abort / break-stop, plus item-drop and a few miscellaneous
/// actions. Per ADR 0002, verified against `javap -p` of the
/// unobfuscated jar: `ServerboundPlayerActionPacket(action, pos,
/// direction, sequence)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundPlayerAction {
    pub action: PlayerActionKind,
    /// Block position packed into an `i64` per the vanilla
    /// `BlockPos.toLong` format used elsewhere in this module
    /// (`SetDefaultSpawnPosition`, `BlockUpdate`).
    pub position: i64,
    pub direction: Direction,
    pub sequence: i32,
}

/// Local 26.1.2 `ServerboundPlayerCommandPacket$Action.values()` order.
/// Shift-key state is carried by `ServerboundPlayerInput`, not this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerCommandAction {
    StopSleeping = 0,
    StartSprinting = 1,
    StopSprinting = 2,
    StartRidingJump = 3,
    StopRidingJump = 4,
    OpenInventory = 5,
    StartFallFlying = 6,
}

impl PlayerCommandAction {
    fn from_wire(value: i32) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::StopSleeping,
            1 => Self::StartSprinting,
            2 => Self::StopSprinting,
            3 => Self::StartRidingJump,
            4 => Self::StopRidingJump,
            5 => Self::OpenInventory,
            6 => Self::StartFallFlying,
            other => {
                return Err(CodecError::StringTooLong {
                    len: other as usize,
                    max: 6,
                });
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundPlayerCommand {
    pub entity_id: i32,
    pub action: PlayerCommandAction,
    pub data: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlayerInput {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub shift: bool,
    pub sprint: bool,
}

impl PlayerInput {
    #[must_use]
    pub const fn from_flags(flags: u8) -> Self {
        Self {
            forward: flags & 0x01 != 0,
            backward: flags & 0x02 != 0,
            left: flags & 0x04 != 0,
            right: flags & 0x08 != 0,
            jump: flags & 0x10 != 0,
            shift: flags & 0x20 != 0,
            sprint: flags & 0x40 != 0,
        }
    }

    #[must_use]
    pub const fn flags(self) -> u8 {
        (self.forward as u8)
            | ((self.backward as u8) << 1)
            | ((self.left as u8) << 2)
            | ((self.right as u8) << 3)
            | ((self.jump as u8) << 4)
            | ((self.shift as u8) << 5)
            | ((self.sprint as u8) << 6)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServerboundPlayerInput {
    pub input: PlayerInput,
}

impl Packet for ServerboundPlayerInput {
    // `.analysis/protocol-dump.txt`: SERVERBOUND_PLAYER_INPUT follows
    // SERVERBOUND_PLAYER_COMMAND at game-SB index 43, wire id 0x2B. `javap -p -c`
    // of `ServerboundPlayerInputPacket` and `Input$1` shows one signed byte bitset:
    // forward, backward, left, right, jump, shift, sprint.
    const ID: i32 = 0x2B;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_u8(self.input.flags());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            input: PlayerInput::from_flags(buf.read_u8()?),
        })
    }
}

impl Packet for ServerboundPlayerCommand {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_PLAYER_COMMAND is
    // serverbound registration index 42, wire id 0x2A. The packet record fields
    // are `int id`, `Action action`, then `int data`; vanilla writes all three as
    // VarInts.
    const ID: i32 = 0x2A;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_varint(self.action as i32);
        buf.write_varint(self.data);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            entity_id: buf.read_varint()?,
            action: PlayerCommandAction::from_wire(buf.read_varint()?)?,
            data: buf.read_varint()?,
        })
    }
}

impl Packet for ServerboundPlayerAction {
    // 26.3 `packets.json`: play serverbound `minecraft:player_action` = 41.
    const ID: i32 = 0x29;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.action as i32);
        buf.write_i64(self.position);
        buf.write_varint(self.direction as i32);
        buf.write_varint(self.sequence);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let action = PlayerActionKind::from_wire(buf.read_varint()?)?;
        let position = buf.read_i64()?;
        let direction = direction_from_wire(buf.read_varint()?)?;
        let sequence = buf.read_varint()?;
        Ok(Self {
            action,
            position,
            direction,
            sequence,
        })
    }
}

/// 26.3 `ServerboundPunchPacket` (`StreamCodec.unit`): a left click that hit
/// nothing. It replaced the 26.1.2 serverbound swing packet; vanilla swings
/// the main hand and resets the attack-strength ticker.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServerboundPunch;

impl Packet for ServerboundPunch {
    // 26.3 `packets.json`: play serverbound `minecraft:punch` = 46.
    const ID: i32 = 0x2E;

    fn encode<B: BufMut>(&self, _buf: &mut B) -> Result<(), CodecError> {
        Ok(())
    }

    fn decode<B: Buf>(_buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self)
    }
}

/// `Serverbound Use Item On` (SB) — the place-block / right-click
/// packet. Per ADR 0002, verified against `javap -p`:
/// `ServerboundUseItemOnPacket(BlockHitResult blockHit,
/// InteractionHand hand, int sequence)`.
///
/// The wire order, per the unobfuscated jar's stream codec, is
/// `hand` (VarInt) first, then the `BlockHitResult` body, then
/// `sequence`. `BlockHitResult` on the wire is *not* the in-memory
/// shape — it ships as
/// `(BlockPos pos, Direction direction, Vec3 cursor, bool inside,
/// bool world_border_hit)` (`pos` packed as the standard `i64`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerboundUseItemOn {
    pub hand: InteractionHand,
    pub position: i64,
    pub direction: Direction,
    pub cursor_x: f32,
    pub cursor_y: f32,
    pub cursor_z: f32,
    pub inside: bool,
    pub world_border_hit: bool,
    pub sequence: i32,
}

impl Packet for ServerboundUseItemOn {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // SERVERBOUND_USE_ITEM_ON sits at game-SB index 66 = wire id 0x42.
    const ID: i32 = 0x42;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.hand as i32);
        buf.write_i64(self.position);
        buf.write_varint(self.direction as i32);
        buf.write_f32(self.cursor_x);
        buf.write_f32(self.cursor_y);
        buf.write_f32(self.cursor_z);
        buf.write_bool(self.inside);
        buf.write_bool(self.world_border_hit);
        buf.write_varint(self.sequence);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let hand = interaction_hand_from_wire(buf.read_varint()?)?;
        let position = buf.read_i64()?;
        let direction = direction_from_wire(buf.read_varint()?)?;
        let cursor_x = buf.read_f32()?;
        let cursor_y = buf.read_f32()?;
        let cursor_z = buf.read_f32()?;
        let inside = buf.read_bool()?;
        let world_border_hit = buf.read_bool()?;
        let sequence = buf.read_varint()?;
        Ok(Self {
            hand,
            position,
            direction,
            cursor_x,
            cursor_y,
            cursor_z,
            inside,
            world_border_hit,
            sequence,
        })
    }
}

/// `Serverbound Use Item` (SB) — right-click air / consume held item.
/// Verified via `.analysis/protocol-dump.txt` and `javap -p`:
/// `ServerboundUseItemPacket(InteractionHand hand, int sequence,
/// float yRot, float xRot)`; game-SB index 67 = wire id 0x43.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerboundUseItem {
    pub hand: InteractionHand,
    pub sequence: i32,
    pub y_rot: f32,
    pub x_rot: f32,
}

impl Packet for ServerboundUseItem {
    const ID: i32 = 0x43;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.hand as i32);
        buf.write_varint(self.sequence);
        buf.write_f32(self.y_rot);
        buf.write_f32(self.x_rot);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            hand: interaction_hand_from_wire(buf.read_varint()?)?,
            sequence: buf.read_varint()?,
            y_rot: buf.read_f32()?,
            x_rot: buf.read_f32()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientCommandAction {
    PerformRespawn,
    RequestStats,
    RequestGameruleValues,
}

impl ClientCommandAction {
    fn from_wire(v: i32) -> Result<Self, CodecError> {
        Ok(match v {
            0 => Self::PerformRespawn,
            1 => Self::RequestStats,
            2 => Self::RequestGameruleValues,
            other => {
                return Err(CodecError::StringTooLong {
                    len: other as usize,
                    max: 2,
                });
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundClientCommand {
    pub action: ClientCommandAction,
}

impl Packet for ServerboundClientCommand {
    // SERVERBOUND_CLIENT_COMMAND is game-SB index 12 = wire id 0x0C;
    // Action enum ordinals are PERFORM_RESPAWN=0, REQUEST_STATS=1,
    // REQUEST_GAMERULE_VALUES=2 per local javap.
    const ID: i32 = 0x0C;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.action as i32);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            action: ClientCommandAction::from_wire(buf.read_varint()?)?,
        })
    }
}

// -----------------------------------------------------------------------
// Tests
