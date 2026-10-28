//! Play chat and command packets: chat, commands, suggestions, tab list, tick sync.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundChangeGameMode {
    pub mode: GameMode,
}

impl Packet for ServerboundChangeGameMode {
    // Verified from `.analysis/protocol-dump.txt`: SERVERBOUND_CHANGE_GAME_MODE
    // is game-SB index 5 = wire id 0x05. `javap -p -c
    // ServerboundChangeGameModePacket` shows the body is GameType.STREAM_CODEC;
    // `javap GameType` shows ids survival=0, creative=1, adventure=2,
    // spectator=3 via ByteBufCodecs.idMapper.
    const ID: i32 = 0x05;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.mode.id());
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            mode: GameMode::from_id(buf.read_varint()?),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundChatCommand {
    pub command: String,
}

impl Packet for ServerboundChatCommand {
    // Verified from `.analysis/protocol-dump.txt`: SERVERBOUND_CHAT_COMMAND is
    // game-SB index 7 = wire id 0x07. `javap -p -c
    // ServerboundChatCommandPacket` shows a single FriendlyByteBuf UTF string.
    const ID: i32 = 0x07;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_string(&self.command, MAX_COMMAND_LEN)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            command: buf.read_string(MAX_COMMAND_LEN)?,
        })
    }
}

/// 26.1.2 `SERVERBOUND_CHAT_COMMAND_SIGNED` (game-SB index 8).
/// Layout: local ServerboundChatCommandSignedPacket, ArgumentSignatures and
/// LastSeenMessages.Update. Transport support does not imply signature trust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundChatCommandSigned {
    pub command: String,
    pub timestamp_millis: i64,
    pub salt: i64,
    pub signatures: Vec<(String, [u8; 256])>,
    pub last_seen_offset: i32,
    pub last_seen_acknowledged: [u8; LAST_SEEN_FIXED_BITSET_BYTES],
    pub last_seen_checksum: i8,
}

impl Packet for ServerboundChatCommandSigned {
    const ID: i32 = 0x08;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.signatures.len() > 8 {
            return Err(CodecError::StringTooLong {
                len: self.signatures.len(),
                max: 8,
            });
        }
        buf.write_string(&self.command, MAX_COMMAND_LEN)?;
        buf.write_i64(self.timestamp_millis);
        buf.write_i64(self.salt);
        buf.write_varint(self.signatures.len() as i32);
        for (name, signature) in &self.signatures {
            buf.write_string(name, 16)?;
            buf.put_slice(signature);
        }
        buf.write_varint(self.last_seen_offset);
        buf.put_slice(&self.last_seen_acknowledged);
        buf.write_i8(self.last_seen_checksum);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let command = buf.read_string(MAX_COMMAND_LEN)?;
        let timestamp_millis = buf.read_i64()?;
        let salt = buf.read_i64()?;
        let signatures = read_bounded_vec(buf, 8, 257, |buf| {
            let name = buf.read_string(16)?;
            let mut signature = [0; 256];
            for byte in &mut signature {
                *byte = buf.read_u8()?;
            }
            Ok((name, signature))
        })?;
        let last_seen_offset = buf.read_varint()?;
        let mut last_seen_acknowledged = [0; LAST_SEEN_FIXED_BITSET_BYTES];
        for byte in &mut last_seen_acknowledged {
            *byte = buf.read_u8()?;
        }
        Ok(Self {
            command,
            timestamp_millis,
            salt,
            signatures,
            last_seen_offset,
            last_seen_acknowledged,
            last_seen_checksum: buf.read_i8()?,
        })
    }
}

/// Local 26.1.2 GameProtocols index 10; RemoteChatSession.Data and
/// ProfilePublicKey.Data: UUID, expiry millis, DER key (512), certificate (4096).
/// This is an untrusted transport value, not a verified identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundChatSessionUpdate {
    pub session_id: Uuid,
    pub expires_at_millis: i64,
    pub public_key: Vec<u8>,
    pub key_signature: Vec<u8>,
}

impl Packet for ServerboundChatSessionUpdate {
    const ID: i32 = 0x0a;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        for (len, max) in [
            (self.public_key.len(), 512),
            (self.key_signature.len(), 4096),
        ] {
            if len > max {
                return Err(CodecError::StringTooLong { len, max });
            }
        }
        buf.write_uuid(self.session_id);
        buf.write_i64(self.expires_at_millis);
        buf.write_byte_array(&self.public_key)?;
        buf.write_byte_array(&self.key_signature)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            session_id: buf.read_uuid()?,
            expires_at_millis: buf.read_i64()?,
            public_key: buf.read_byte_array(512)?,
            key_signature: buf.read_byte_array(4096)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundChat {
    pub message: String,
    pub timestamp_millis: i64,
    pub salt: i64,
    pub signature: Option<[u8; 256]>,
    pub last_seen_offset: i32,
    pub last_seen_acknowledged: [u8; LAST_SEEN_FIXED_BITSET_BYTES],
    pub last_seen_checksum: i8,
}

impl Packet for ServerboundChat {
    // Verified from `.analysis/protocol-dump.txt`: SERVERBOUND_CHAT is
    // game-SB index 9 = wire id 0x09. Local decompiled
    // `ServerboundChatPacket` reads UTF(256), Instant as epoch millis, salt,
    // nullable MessageSignature (256 bytes), then LastSeenMessages.Update:
    // VarInt offset, fixed 20-bit BitSet, and checksum byte.
    const ID: i32 = 0x09;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_string(&self.message, MAX_CHAT_MESSAGE_LEN)?;
        buf.write_i64(self.timestamp_millis);
        buf.write_i64(self.salt);
        match self.signature {
            Some(signature) => {
                buf.write_bool(true);
                buf.put_slice(&signature);
            }
            None => buf.write_bool(false),
        }
        buf.write_varint(self.last_seen_offset);
        buf.put_slice(&self.last_seen_acknowledged);
        buf.write_i8(self.last_seen_checksum);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let message = buf.read_string(MAX_CHAT_MESSAGE_LEN)?;
        let timestamp_millis = buf.read_i64()?;
        let salt = buf.read_i64()?;
        let signature = if buf.read_bool()? {
            let mut signature = [0_u8; 256];
            if buf.remaining() < signature.len() {
                return Err(CodecError::Underflow {
                    needed: signature.len() - buf.remaining(),
                    available: buf.remaining(),
                });
            }
            buf.copy_to_slice(&mut signature);
            Some(signature)
        } else {
            None
        };
        let last_seen_offset = buf.read_varint()?;
        let mut last_seen_acknowledged = [0_u8; LAST_SEEN_FIXED_BITSET_BYTES];
        if buf.remaining() < last_seen_acknowledged.len() {
            return Err(CodecError::Underflow {
                needed: last_seen_acknowledged.len() - buf.remaining(),
                available: buf.remaining(),
            });
        }
        buf.copy_to_slice(&mut last_seen_acknowledged);
        Ok(Self {
            message,
            timestamp_millis,
            salt,
            signature,
            last_seen_offset,
            last_seen_acknowledged,
            last_seen_checksum: buf.read_i8()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundCommandSuggestion {
    pub id: i32,
    pub command: String,
}

impl Packet for ServerboundCommandSuggestion {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_COMMAND_SUGGESTION is
    // serverbound registration index 15, wire id 0x0F. Local decompiled source
    // reads VarInt id then `readUtf(32500)`.
    const ID: i32 = 0x0F;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        let command_len = self.command.encode_utf16().count();
        if command_len > MAX_COMMAND_SUGGESTION_LEN {
            return Err(CodecError::StringTooLong {
                len: command_len,
                max: MAX_COMMAND_SUGGESTION_LEN,
            });
        }
        buf.write_varint(self.id);
        buf.write_string(&self.command, MAX_COMMAND_SUGGESTION_LEN)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            id: buf.read_varint()?,
            command: buf.read_string(MAX_COMMAND_SUGGESTION_LEN)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundSignUpdate {
    pub position: i64,
    pub lines: Vec<String>,
    pub is_front_text: bool,
}

impl Packet for ServerboundSignUpdate {
    // 26.3 `ServerboundSignUpdatePacket.STREAM_CODEC`: BlockPos, four
    // UTF(384) lines (`fixedSizeList(4)`), then `SignTextSlot` (VarInt
    // 0 = back, 1 = front).
    const ID: i32 = 0x3E;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.lines.len() != SIGN_LINE_COUNT {
            return Err(CodecError::NotSupported(
                "sign update must contain four lines",
            ));
        }
        for line in &self.lines {
            let line_len = line.encode_utf16().count();
            if line_len > MAX_SIGN_LINE_LEN {
                return Err(CodecError::StringTooLong {
                    len: line_len,
                    max: MAX_SIGN_LINE_LEN,
                });
            }
        }
        buf.write_i64(self.position);
        for line in &self.lines {
            buf.write_string(line, MAX_SIGN_LINE_LEN)?;
        }
        buf.write_varint(i32::from(self.is_front_text));
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let position = buf.read_i64()?;
        let mut lines = Vec::with_capacity(SIGN_LINE_COUNT);
        for _ in 0..SIGN_LINE_COUNT {
            lines.push(buf.read_string(MAX_SIGN_LINE_LEN)?);
        }
        let is_front_text = match buf.read_varint()? {
            0 => false,
            1 => true,
            _ => return Err(CodecError::NotSupported("unknown sign text slot")),
        };
        Ok(Self {
            position,
            lines,
            is_front_text,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundEditBook {
    pub slot: i32,
    pub pages: Vec<String>,
    pub title: Option<String>,
}
impl Packet for ServerboundEditBook {
    // Local reports/packets.json: edit_book=24. Local javap codec:
    // VAR_INT, list(100,stringUtf8(1024)), optional(stringUtf8(32)).
    const ID: i32 = 24;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.pages.len() > 100 {
            return Err(CodecError::NotSupported("too many book pages"));
        }
        for (value, max) in self
            .pages
            .iter()
            .map(|page| (page, 1024))
            .chain(self.title.iter().map(|title| (title, 32)))
        {
            let len = value.encode_utf16().count();
            if len > max {
                return Err(CodecError::StringTooLong { len, max });
            }
        }
        buf.write_varint(self.slot);
        write_count(buf, self.pages.len())?;
        for page in &self.pages {
            buf.write_string(page, 1024)?;
        }
        buf.write_bool(self.title.is_some());
        if let Some(title) = &self.title {
            buf.write_string(title, 32)?;
        }
        Ok(())
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let slot = buf.read_varint()?;
        let pages = read_bounded_vec(buf, 100, 1, |buf| buf.read_string(1024))?;
        let title = if buf.read_bool()? {
            Some(buf.read_string(32)?)
        } else {
            None
        };
        Ok(Self { slot, pages, title })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundOpenBook {
    pub hand: InteractionHand,
}
impl Packet for ClientboundOpenBook {
    // Local reports/packets.json: open_book=58; javap writes InteractionHand enum.
    const ID: i32 = 0x3B;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.hand as i32);
        Ok(())
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            hand: interaction_hand_from_wire(buf.read_varint()?)?,
        })
    }
}

#[cfg(test)]
#[path = "book_packet_tests.rs"]
mod book_tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundCommandSuggestions {
    pub id: i32,
    pub start: i32,
    pub length: i32,
    pub suggestions: Vec<CommandSuggestionEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundCommands {
    pub nodes: Vec<CommandNode>,
    pub root_index: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandNode {
    pub kind: CommandNodeKind,
    pub children: Vec<i32>,
    pub executable: bool,
    pub restricted: bool,
    pub redirect: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandNodeKind {
    Root,
    Literal(String),
    Argument {
        name: String,
        parser: CommandArgumentParser,
        suggestions: Option<Identifier>,
    },
}

/// Vanilla 26.1.2 command argument types and their network properties.
///
/// Numeric `None` bounds use Brigadier's type-specific extremes. Floating-point
/// bounds store IEEE-754 bits so command packets retain exact equality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandArgumentParser {
    Bool,
    Float {
        min: Option<u32>,
        max: Option<u32>,
    },
    Double {
        min: Option<u64>,
        max: Option<u64>,
    },
    Integer {
        min: Option<i32>,
        max: Option<i32>,
    },
    Long {
        min: Option<i64>,
        max: Option<i64>,
    },
    String(CommandStringKind),
    Entity {
        single: bool,
        players_only: bool,
    },
    GameProfile,
    BlockPos,
    ColumnPos,
    Vec3,
    Vec2,
    BlockState,
    BlockPredicate,
    ItemStack,
    ItemPredicate,
    Color,
    HexColor,
    Component,
    Style,
    Message,
    NbtCompoundTag,
    NbtTag,
    NbtPath,
    Objective,
    ObjectiveCriteria,
    Operation,
    Angle,
    Rotation,
    ScoreboardSlot,
    ScoreHolder {
        multiple: bool,
    },
    Swizzle,
    Team,
    ResourceLocation,
    EntityAnchor,
    Dimension,
    GameMode,
    Time {
        min: i32,
    },
    /// `minecraft:resource_or_tag` (ArgumentTypeInfos id 44).
    ResourceOrTag {
        registry: Identifier,
    },
    /// `minecraft:resource_or_tag_key` (ArgumentTypeInfos id 45).
    ResourceOrTagKey {
        registry: Identifier,
    },
    Resource {
        registry: Identifier,
    },
    ResourceKey {
        registry: Identifier,
    },
    Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandStringKind {
    SingleWord,
    QuotablePhrase,
    GreedyPhrase,
}

impl CommandNode {
    pub fn root(children: Vec<i32>) -> Self {
        Self {
            kind: CommandNodeKind::Root,
            children,
            executable: false,
            restricted: false,
            redirect: None,
        }
    }

    pub fn literal(name: impl Into<String>, children: Vec<i32>, executable: bool) -> Self {
        Self {
            kind: CommandNodeKind::Literal(name.into()),
            children,
            executable,
            restricted: false,
            redirect: None,
        }
    }

    pub fn argument(
        name: impl Into<String>,
        parser: CommandArgumentParser,
        children: Vec<i32>,
        executable: bool,
    ) -> Self {
        Self {
            kind: CommandNodeKind::Argument {
                name: name.into(),
                parser,
                suggestions: None,
            },
            children,
            executable,
            restricted: false,
            redirect: None,
        }
    }

    pub fn restricted(mut self, restricted: bool) -> Self {
        self.restricted = restricted;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSuggestionEntry {
    pub text: String,
    pub tooltip_nbt: Option<Vec<u8>>,
}

impl Packet for ClientboundCommandSuggestions {
    // `.analysis/protocol-dump.txt`: game CLIENTBOUND_COMMAND_SUGGESTIONS is
    // clientbound registration index 15, wire id 0x0F. Local decompiled source
    // writes VarInt id/start/length and a list of entries. Solaris currently
    // emits empty lists, so optional component tooltips are not produced.
    const ID: i32 = 0x0F;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.id);
        buf.write_varint(self.start);
        buf.write_varint(self.length);
        write_count(buf, self.suggestions.len())?;
        for entry in &self.suggestions {
            buf.write_string(&entry.text, MAX_COMMAND_LEN)?;
            match &entry.tooltip_nbt {
                Some(tooltip) => {
                    buf.write_bool(true);
                    buf.put_slice(tooltip);
                }
                None => buf.write_bool(false),
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let id = buf.read_varint()?;
        let start = buf.read_varint()?;
        let length = buf.read_varint()?;
        let suggestions = read_bounded_vec(buf, 256, 2, |buf| {
            let text = buf.read_string(MAX_COMMAND_LEN)?;
            let tooltip_nbt = if buf.read_bool()? {
                let mut tooltip = vec![0; buf.remaining()];
                buf.copy_to_slice(&mut tooltip);
                Some(tooltip)
            } else {
                None
            };
            Ok(CommandSuggestionEntry { text, tooltip_nbt })
        })?;
        Ok(Self {
            id,
            start,
            length,
            suggestions,
        })
    }
}

impl Packet for ClientboundCommands {
    // `.analysis/protocol-dump.txt` / `GameProtocols`: game
    // CLIENTBOUND_COMMANDS is clientbound registration index 16, wire id 0x10.
    // `ClientboundCommandsPacket` writes a list of nodes followed by rootIndex;
    // each node is flags, VarInt child array, optional redirect, then the
    // literal or argument payload selected by flags & 3.
    const ID: i32 = 0x10;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_count(buf, self.nodes.len())?;
        for node in &self.nodes {
            node.encode(buf)?;
        }
        buf.write_varint(self.root_index);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let nodes = read_bounded_vec(buf, MAX_COMMAND_NODE_COUNT, 2, CommandNode::decode)?;
        Ok(Self {
            nodes,
            root_index: buf.read_varint()?,
        })
    }
}

impl CommandNode {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        let mut flags = match self.kind {
            CommandNodeKind::Root => 0,
            CommandNodeKind::Literal(_) => 1,
            CommandNodeKind::Argument { .. } => 2,
        };
        if self.executable {
            flags |= 4;
        }
        if self.redirect.is_some() {
            flags |= 8;
        }
        if matches!(
            self.kind,
            CommandNodeKind::Argument {
                suggestions: Some(_),
                ..
            }
        ) {
            flags |= 16;
        }
        if self.restricted {
            flags |= 32;
        }

        buf.put_u8(flags);
        write_count(buf, self.children.len())?;
        for &child in &self.children {
            buf.write_varint(child);
        }
        if let Some(redirect) = self.redirect {
            buf.write_varint(redirect);
        }
        match &self.kind {
            CommandNodeKind::Root => {}
            CommandNodeKind::Literal(name) => buf.write_string(name, MAX_COMMAND_LEN)?,
            CommandNodeKind::Argument {
                name,
                parser,
                suggestions,
            } => {
                buf.write_string(name, MAX_COMMAND_LEN)?;
                parser.encode(buf)?;
                if let Some(suggestions) = suggestions {
                    buf.write_identifier(suggestions)?;
                }
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let flags = buf.read_u8()?;
        let children = read_bounded_vec(buf, MAX_COMMAND_CHILD_COUNT, 1, ReadMc::read_varint)?;
        let redirect = if flags & 8 != 0 {
            Some(buf.read_varint()?)
        } else {
            None
        };
        let suggestions = flags & 16 != 0;
        let kind = match flags & 3 {
            0 => CommandNodeKind::Root,
            1 => CommandNodeKind::Literal(buf.read_string(MAX_COMMAND_LEN)?),
            2 => {
                let name = buf.read_string(MAX_COMMAND_LEN)?;
                let parser = CommandArgumentParser::decode(buf)?;
                let suggestions = suggestions.then(|| buf.read_identifier()).transpose()?;
                CommandNodeKind::Argument {
                    name,
                    parser,
                    suggestions,
                }
            }
            _ => return Err(CodecError::NotSupported("unknown command node type")),
        };
        Ok(Self {
            kind,
            children,
            executable: flags & 4 != 0,
            restricted: flags & 32 != 0,
            redirect,
        })
    }
}

impl CommandArgumentParser {
    // 26.3 `minecraft:command_argument_type` registry order (registries.json).
    // `minecraft:color` became `minecraft:team_color` at the same id 16 (both
    // singletons); `minecraft:uuid` moved to 61. SingletonArgumentInfo has no
    // property payload. Numeric ArgumentInfos, EntityArgument.Info,
    // ScoreHolderArgument.Info, TimeArgument.Info, and Resource[Key]Argument.Info
    // define the property layouts below.
    fn id(&self) -> i32 {
        match self {
            Self::Bool => 0,
            Self::Float { .. } => 1,
            Self::Double { .. } => 2,
            Self::Integer { .. } => 3,
            Self::Long { .. } => 4,
            Self::String(_) => 5,
            Self::Entity { .. } => 6,
            Self::GameProfile => 7,
            Self::BlockPos => 8,
            Self::ColumnPos => 9,
            Self::Vec3 => 10,
            Self::Vec2 => 11,
            Self::BlockState => 12,
            Self::BlockPredicate => 13,
            Self::ItemStack => 14,
            Self::ItemPredicate => 15,
            Self::Color => 16,
            Self::HexColor => 17,
            Self::Component => 18,
            Self::Style => 19,
            Self::Message => 20,
            Self::NbtCompoundTag => 21,
            Self::NbtTag => 22,
            Self::NbtPath => 23,
            Self::Objective => 24,
            Self::ObjectiveCriteria => 25,
            Self::Operation => 26,
            Self::Angle => 28,
            Self::Rotation => 29,
            Self::ScoreboardSlot => 30,
            Self::ScoreHolder { .. } => 31,
            Self::Swizzle => 32,
            Self::Team => 33,
            Self::ResourceLocation => 36,
            Self::EntityAnchor => 38,
            Self::Dimension => 41,
            Self::GameMode => 42,
            Self::Time { .. } => 43,
            Self::ResourceOrTag { .. } => 44,
            Self::ResourceOrTagKey { .. } => 45,
            Self::Resource { .. } => 46,
            Self::ResourceKey { .. } => 47,
            Self::Uuid => 61,
        }
    }

    fn validate(&self) -> Result<(), CodecError> {
        let valid = match self {
            Self::Integer { min, max } => min.unwrap_or(i32::MIN) <= max.unwrap_or(i32::MAX),
            Self::Long { min, max } => min.unwrap_or(i64::MIN) <= max.unwrap_or(i64::MAX),
            Self::Float { min, max } => {
                let min = min.map(f32::from_bits).unwrap_or(-f32::MAX);
                let max = max.map(f32::from_bits).unwrap_or(f32::MAX);
                min.is_finite() && max.is_finite() && min <= max
            }
            Self::Double { min, max } => {
                let min = min.map(f64::from_bits).unwrap_or(-f64::MAX);
                let max = max.map(f64::from_bits).unwrap_or(f64::MAX);
                min.is_finite() && max.is_finite() && min <= max
            }
            _ => true,
        };
        if valid {
            Ok(())
        } else {
            Err(CodecError::NotSupported("invalid command numeric bounds"))
        }
    }

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.validate()?;
        buf.write_varint(self.id());
        match self {
            Self::Float { min, max } => {
                buf.write_u8(u8::from(min.is_some()) | (u8::from(max.is_some()) << 1));
                if let Some(min) = min {
                    buf.write_u32(*min);
                }
                if let Some(max) = max {
                    buf.write_u32(*max);
                }
            }
            Self::Double { min, max } => {
                buf.write_u8(u8::from(min.is_some()) | (u8::from(max.is_some()) << 1));
                if let Some(min) = min {
                    buf.write_u64(*min);
                }
                if let Some(max) = max {
                    buf.write_u64(*max);
                }
            }
            Self::Integer { min, max } => {
                buf.write_u8(u8::from(min.is_some()) | (u8::from(max.is_some()) << 1));
                if let Some(min) = min {
                    buf.write_i32(*min);
                }
                if let Some(max) = max {
                    buf.write_i32(*max);
                }
            }
            Self::Long { min, max } => {
                buf.write_u8(u8::from(min.is_some()) | (u8::from(max.is_some()) << 1));
                if let Some(min) = min {
                    buf.write_i64(*min);
                }
                if let Some(max) = max {
                    buf.write_i64(*max);
                }
            }
            Self::String(kind) => buf.write_varint(*kind as i32),
            Self::Entity {
                single,
                players_only,
            } => {
                buf.write_u8(u8::from(*single) | (u8::from(*players_only) << 1));
            }
            Self::ScoreHolder { multiple } => buf.write_u8(u8::from(*multiple)),
            Self::Time { min } => buf.write_i32(*min),
            Self::ResourceOrTag { registry }
            | Self::ResourceOrTagKey { registry }
            | Self::Resource { registry }
            | Self::ResourceKey { registry } => {
                buf.write_identifier(registry)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn read_flags<B: Buf>(buf: &mut B, mask: u8) -> Result<u8, CodecError> {
        let flags = buf.read_u8()?;
        if flags & !mask != 0 {
            return Err(CodecError::NotSupported("invalid command argument flags"));
        }
        Ok(flags)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let parser = match buf.read_varint()? {
            0 => Self::Bool,
            1 => {
                let flags = Self::read_flags(buf, 3)?;
                Self::Float {
                    min: (flags & 1 != 0).then(|| buf.read_u32()).transpose()?,
                    max: (flags & 2 != 0).then(|| buf.read_u32()).transpose()?,
                }
            }
            2 => {
                let flags = Self::read_flags(buf, 3)?;
                Self::Double {
                    min: (flags & 1 != 0).then(|| buf.read_u64()).transpose()?,
                    max: (flags & 2 != 0).then(|| buf.read_u64()).transpose()?,
                }
            }
            3 => {
                let flags = Self::read_flags(buf, 3)?;
                Self::Integer {
                    min: (flags & 1 != 0).then(|| buf.read_i32()).transpose()?,
                    max: (flags & 2 != 0).then(|| buf.read_i32()).transpose()?,
                }
            }
            4 => {
                let flags = Self::read_flags(buf, 3)?;
                Self::Long {
                    min: (flags & 1 != 0).then(|| buf.read_i64()).transpose()?,
                    max: (flags & 2 != 0).then(|| buf.read_i64()).transpose()?,
                }
            }
            5 => Self::String(CommandStringKind::decode(buf)?),
            6 => {
                let flags = Self::read_flags(buf, 3)?;
                Self::Entity {
                    single: flags & 1 != 0,
                    players_only: flags & 2 != 0,
                }
            }
            7 => Self::GameProfile,
            8 => Self::BlockPos,
            9 => Self::ColumnPos,
            10 => Self::Vec3,
            11 => Self::Vec2,
            12 => Self::BlockState,
            13 => Self::BlockPredicate,
            14 => Self::ItemStack,
            15 => Self::ItemPredicate,
            16 => Self::Color,
            17 => Self::HexColor,
            18 => Self::Component,
            19 => Self::Style,
            20 => Self::Message,
            21 => Self::NbtCompoundTag,
            22 => Self::NbtTag,
            23 => Self::NbtPath,
            24 => Self::Objective,
            25 => Self::ObjectiveCriteria,
            26 => Self::Operation,
            28 => Self::Angle,
            29 => Self::Rotation,
            30 => Self::ScoreboardSlot,
            31 => Self::ScoreHolder {
                multiple: Self::read_flags(buf, 1)? != 0,
            },
            32 => Self::Swizzle,
            33 => Self::Team,
            36 => Self::ResourceLocation,
            38 => Self::EntityAnchor,
            41 => Self::Dimension,
            42 => Self::GameMode,
            43 => Self::Time {
                min: buf.read_i32()?,
            },
            44 => Self::ResourceOrTag {
                registry: buf.read_identifier()?,
            },
            45 => Self::ResourceOrTagKey {
                registry: buf.read_identifier()?,
            },
            46 => Self::Resource {
                registry: buf.read_identifier()?,
            },
            47 => Self::ResourceKey {
                registry: buf.read_identifier()?,
            },
            61 => Self::Uuid,
            _ => {
                return Err(CodecError::NotSupported(
                    "unsupported command argument parser",
                ));
            }
        };
        parser.validate()?;
        Ok(parser)
    }
}

impl CommandStringKind {
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        match buf.read_varint()? {
            0 => Ok(Self::SingleWord),
            1 => Ok(Self::QuotablePhrase),
            2 => Ok(Self::GreedyPhrase),
            _ => Err(CodecError::NotSupported(
                "unsupported brigadier string kind",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundSystemChat {
    pub content_nbt: Vec<u8>,
    pub overlay: bool,
}

impl Packet for ClientboundSystemChat {
    // `.analysis/protocol-dump.txt` / `GameProtocols`: game
    // CLIENTBOUND_SYSTEM_CHAT is clientbound registration index 121, wire id
    // 0x79. `ClientboundSystemChatPacket` writes trusted Component NBT then a
    // bool overlay flag.
    const ID: i32 = 0x7C;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.put_slice(&self.content_nbt);
        buf.write_bool(self.overlay);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        if buf.remaining() < 2 {
            return Err(CodecError::Underflow {
                needed: 2 - buf.remaining(),
                available: buf.remaining(),
            });
        }
        let content_len = buf.remaining() - 1;
        let mut content_nbt = vec![0; content_len];
        buf.copy_to_slice(&mut content_nbt);
        Ok(Self {
            content_nbt,
            overlay: buf.read_bool()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundTabList {
    /// Pre-serialised trusted-`Component` NBT for the header line block.
    pub header_nbt: Vec<u8>,
    /// Pre-serialised trusted-`Component` NBT for the footer line block.
    pub footer_nbt: Vec<u8>,
}

impl Packet for ClientboundTabList {
    // Verified against the local vanilla 26.1.2 jars: `GameProtocols`
    // registers CLIENTBOUND_TAB_LIST at clientbound index 122 = wire id
    // 0x7A (between CLIENTBOUND_SYSTEM_CHAT at 0x79 and
    // CLIENTBOUND_TAG_QUERY), and `ClientboundTabListPacket` is a
    // record of two `Component`s written with the trusted NBT stream codec.
    const ID: i32 = 0x7D;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.put_slice(&self.header_nbt);
        buf.put_slice(&self.footer_nbt);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let header = mc_nbt::read_network(buf)?;
        let footer = mc_nbt::read_network(buf)?;
        let mut header_nbt = Vec::new();
        mc_nbt::write_network(&mut header_nbt, &header)?;
        let mut footer_nbt = Vec::new();
        mc_nbt::write_network(&mut footer_nbt, &footer)?;
        Ok(Self {
            header_nbt,
            footer_nbt,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundChatAck {
    pub offset: i32,
}

impl Packet for ServerboundChatAck {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_CHAT_ACK is serverbound
    // registration index 6, wire id 0x06. Local decompiled source reads one
    // VarInt offset.
    const ID: i32 = 0x06;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.offset);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            offset: buf.read_varint()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerboundChunkBatchReceived {
    pub desired_chunks_per_tick: f32,
}

impl Packet for ServerboundChunkBatchReceived {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_CHUNK_BATCH_RECEIVED is
    // serverbound registration index 11, wire id 0x0B. Local decompiled source
    // reads one f32 desiredChunksPerTick.
    const ID: i32 = 0x0B;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_f32(self.desired_chunks_per_tick);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            desired_chunks_per_tick: buf.read_f32()?,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServerboundClientTickEnd;

impl Packet for ServerboundClientTickEnd {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_CLIENT_TICK_END is
    // serverbound registration index 13, wire id 0x0D. Local decompiled source
    // uses `StreamCodec.unit`, so the body is empty.
    const ID: i32 = 0x0D;

    fn encode<B: BufMut>(&self, _buf: &mut B) -> Result<(), CodecError> {
        Ok(())
    }

    fn decode<B: Buf>(_buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServerboundPlayerLoaded;

impl Packet for ServerboundPlayerLoaded {
    // `.analysis/protocol-dump.txt`: game SERVERBOUND_PLAYER_LOADED is
    // serverbound registration index 44, wire id 0x2C. Local decompiled source
    // uses `StreamCodec.unit`, so the body is empty.
    const ID: i32 = 0x2C;

    fn encode<B: BufMut>(&self, _buf: &mut B) -> Result<(), CodecError> {
        Ok(())
    }

    fn decode<B: Buf>(_buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self)
    }
}
