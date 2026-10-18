//! Play inventory packets: containers, item stacks, recipe book.

use super::*;

/// `Clientbound Set Held Slot` (CB). Tells the client which hotbar
/// slot the server believes is selected. Server emits this on login
/// to seed the cursor. Per ADR 0002, verified against `javap -p`:
/// `ClientboundSetHeldSlotPacket(int slot)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundSetHeldSlot {
    pub slot: i32,
}

impl Packet for ClientboundSetHeldSlot {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_SET_HELD_SLOT at game-CB index 105 = wire id 0x69.
    const ID: i32 = 0x69;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.slot);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            slot: buf.read_varint()?,
        })
    }
}

/// `Clientbound Container Set Content` (CB). Replaces every slot of a
/// container in one packet. M6 uses it on login to seed window 0
/// (player inventory) with the starter kit. Per ADR 0002, verified
/// against `javap -p`:
/// `ClientboundContainerSetContentPacket(int containerId, int stateId,
/// List<ItemStack> items, ItemStack carriedItem)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundContainerSetContent {
    pub container_id: i32,
    pub state_id: i32,
    pub items: Vec<ItemStack>,
    pub carried_item: ItemStack,
}

impl Packet for ClientboundContainerSetContent {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_CONTAINER_SET_CONTENT at game-CB index 18 = wire id 0x12.
    const ID: i32 = 0x12;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        buf.write_varint(self.state_id);
        let len = i32::try_from(self.items.len()).map_err(|_| CodecError::StringTooLong {
            len: self.items.len(),
            max: i32::MAX as usize,
        })?;
        buf.write_varint(len);
        for item in &self.items {
            item.encode(buf)?;
        }
        self.carried_item.encode(buf)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let container_id = buf.read_varint()?;
        let state_id = buf.read_varint()?;
        let len_signed = buf.read_varint()?;
        if len_signed < 0 {
            return Err(CodecError::NegativeLength(len_signed));
        }
        // A vanilla player inventory is 46 slots; allow up to 256 as
        // a soft cap so future container types still fit.
        let len = len_signed as usize;
        if len > 256 {
            return Err(CodecError::StringTooLong { len, max: 256 });
        }
        let mut items = Vec::with_capacity(len);
        for _ in 0..len {
            items.push(ItemStack::decode(buf)?);
        }
        let carried_item = ItemStack::decode(buf)?;
        Ok(Self {
            container_id,
            state_id,
            items,
            carried_item,
        })
    }
}

/// `Clientbound Container Close` (CB). Verified against the local vanilla
/// 26.1.2 class: `ClientboundContainerClosePacket(int containerId)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundContainerClose {
    pub container_id: i32,
}

impl Packet for ClientboundContainerClose {
    const ID: i32 = 0x11;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            container_id: buf.read_varint()?,
        })
    }
}

/// `Clientbound Container Set Data` (CB). Furnace/progress bars use this packet.
/// Verified against the local vanilla 26.1.2 class:
/// `ClientboundContainerSetDataPacket(int containerId, int id, int value)`;
/// `id` and `value` are encoded as shorts on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundContainerSetData {
    pub container_id: i32,
    pub id: i16,
    pub value: i16,
}

impl Packet for ClientboundContainerSetData {
    const ID: i32 = 0x13;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        buf.write_i16(self.id);
        buf.write_i16(self.value);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            container_id: buf.read_varint()?,
            id: buf.read_i16()?,
            value: buf.read_i16()?,
        })
    }
}

/// `Clientbound Container Set Slot` (CB). Single-slot update inside a
/// container. M6 emits one of these after a place mutation to
/// decrement the held stack's count. Per ADR 0002, verified against
/// `javap -p`:
/// `ClientboundContainerSetSlotPacket(int containerId, int stateId,
/// int slot, ItemStack itemStack)`. The Java field type is `int` but
/// the wire codec encodes `slot` as a `short` — same convention as
/// the symmetric serverbound `SetCarriedItem`. M6 missed this and
/// shipped the slot as varint, which slid the downstream `ItemStack`
/// payload and made vanilla 26.1.2 clients fail decode on every
/// post-edit `ContainerSetSlot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundContainerSetSlot {
    pub container_id: i32,
    pub state_id: i32,
    pub slot: i16,
    pub item_stack: ItemStack,
}

impl Packet for ClientboundContainerSetSlot {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_CONTAINER_SET_SLOT at game-CB index 20 = wire id 0x14.
    const ID: i32 = 0x14;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        buf.write_varint(self.state_id);
        buf.write_i16(self.slot);
        self.item_stack.encode(buf)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            container_id: buf.read_varint()?,
            state_id: buf.read_varint()?,
            slot: buf.read_i16()?,
            item_stack: ItemStack::decode(buf)?,
        })
    }
}

/// `Clientbound Set Cursor Item` (CB): the stack carried on the cursor.
/// Verified via `javap` of vanilla 26.1.2 `ClientboundSetCursorItemPacket(ItemStack
/// contents)` (`ItemStack.OPTIONAL_STREAM_CODEC`); `packets.json`
/// `minecraft:set_cursor_item` protocol id 96.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundSetCursorItem {
    pub item_stack: ItemStack,
}

impl Packet for ClientboundSetCursorItem {
    const ID: i32 = 0x60;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.item_stack.encode(buf)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            item_stack: ItemStack::decode(buf)?,
        })
    }
}

/// `Clientbound Open Screen` (CB). Verified against the local vanilla 26.1.2
/// class: `ClientboundOpenScreenPacket(int containerId, MenuType<?> type,
/// Component title)`. The title is an opaque binary text `Component` payload;
/// decode treats it as the final field and consumes the rest of the packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundOpenScreen {
    pub container_id: i32,
    pub menu_type: i32,
    pub title_nbt: Vec<u8>,
}

impl Packet for ClientboundOpenScreen {
    const ID: i32 = 0x3B;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        buf.write_varint(self.menu_type);
        buf.put_slice(&self.title_nbt);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let container_id = buf.read_varint()?;
        let menu_type = buf.read_varint()?;
        let remaining = buf.remaining();
        let mut title_nbt = vec![0; remaining];
        buf.copy_to_slice(&mut title_nbt);
        Ok(Self {
            container_id,
            menu_type,
            title_nbt,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashedStackComponentHashes {
    pub added: Vec<(i32, i32)>,
    pub removed: Vec<i32>,
}

impl HashedStackComponentHashes {
    pub fn empty() -> Self {
        Self {
            added: Vec::new(),
            removed: Vec::new(),
        }
    }

    pub(super) fn component_hash_count(&self) -> Result<usize, CodecError> {
        for len in [self.added.len(), self.removed.len()] {
            if len > MAX_HASHED_STACK_COMPONENT_HASHES {
                return Err(CodecError::StringTooLong {
                    len,
                    max: MAX_HASHED_STACK_COMPONENT_HASHES,
                });
            }
        }
        Ok(self.added.len() + self.removed.len())
    }

    pub(super) fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.component_hash_count()?;

        write_count(buf, self.added.len())?;
        for (component_id, hash) in &self.added {
            buf.write_varint(*component_id);
            buf.write_i32(*hash);
        }

        write_count(buf, self.removed.len())?;
        for component_id in &self.removed {
            buf.write_varint(*component_id);
        }
        Ok(())
    }

    pub(super) fn decode_with_budget<B: Buf>(
        buf: &mut B,
        component_hash_count: &mut usize,
    ) -> Result<Self, CodecError> {
        let added_len = read_count(buf, MAX_HASHED_STACK_COMPONENT_HASHES)?;
        add_component_hashes_to_budget(component_hash_count, added_len)?;
        require_remaining(buf, added_len * 5 + 1)?;
        let mut added = Vec::with_capacity(added_len);
        for _ in 0..added_len {
            added.push((buf.read_varint()?, buf.read_i32()?));
        }

        let removed_len = read_count(buf, MAX_HASHED_STACK_COMPONENT_HASHES)?;
        add_component_hashes_to_budget(component_hash_count, removed_len)?;
        require_remaining(buf, removed_len)?;
        let mut removed = Vec::with_capacity(removed_len);
        for _ in 0..removed_len {
            removed.push(buf.read_varint()?);
        }
        Ok(Self { added, removed })
    }
}

/// Client-side hash view of an item stack, used only by serverbound
/// container reconciliation. The server never trusts these values for
/// inventory mutation; they are decoded so the stream stays aligned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashedStack {
    Empty,
    Actual {
        item_id: u32,
        count: i32,
        components: HashedStackComponentHashes,
    },
}

impl HashedStack {
    pub fn empty() -> Self {
        Self::Empty
    }

    pub(super) fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.component_hash_count()?;
        match self {
            Self::Empty => buf.write_bool(false),
            Self::Actual {
                item_id,
                count,
                components,
            } => {
                buf.write_bool(true);
                buf.write_varint(*item_id as i32);
                buf.write_varint(*count);
                components.encode(buf)?;
            }
        }
        Ok(())
    }

    fn component_hash_count(&self) -> Result<usize, CodecError> {
        match self {
            Self::Empty => Ok(0),
            Self::Actual {
                item_id,
                count,
                components,
            } => {
                if *item_id > i32::MAX as u32 {
                    return Err(CodecError::NotSupported(
                        "hashed stack item id exceeds VarInt range",
                    ));
                }
                if *count <= 0 {
                    return Err(CodecError::NotSupported(
                        "HashedStack actual item with non-positive count",
                    ));
                }
                components.component_hash_count()
            }
        }
    }

    fn decode<B: Buf>(buf: &mut B, component_hash_count: &mut usize) -> Result<Self, CodecError> {
        if !buf.read_bool()? {
            return Ok(Self::Empty);
        }
        let item_id = buf.read_varint()?;
        if item_id < 0 {
            return Err(CodecError::NegativeLength(item_id));
        }
        let count = buf.read_varint()?;
        if count <= 0 {
            return Err(CodecError::NotSupported(
                "HashedStack actual item with non-positive count",
            ));
        }
        Ok(Self::Actual {
            item_id: item_id as u32,
            count,
            components: HashedStackComponentHashes::decode_with_budget(buf, component_hash_count)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerInput {
    Pickup,
    QuickMove,
    Swap,
    Clone,
    Throw,
    QuickCraft,
    PickupAll,
}

impl ContainerInput {
    pub(super) const fn as_wire(self) -> i32 {
        match self {
            Self::Pickup => 0,
            Self::QuickMove => 1,
            Self::Swap => 2,
            Self::Clone => 3,
            Self::Throw => 4,
            Self::QuickCraft => 5,
            Self::PickupAll => 6,
        }
    }

    fn from_wire(value: i32) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::Pickup,
            1 => Self::QuickMove,
            2 => Self::Swap,
            3 => Self::Clone,
            4 => Self::Throw,
            5 => Self::QuickCraft,
            6 => Self::PickupAll,
            _ => return Err(CodecError::NotSupported("unknown ContainerInput id")),
        })
    }
}

/// `Serverbound Container Button Click` (SB). Vanilla 26.1.2 encodes the
/// container id with `ContainerId.STREAM_CODEC`, followed by a VarInt button id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundContainerButtonClick {
    pub container_id: i32,
    pub button_id: i32,
}

impl Packet for ServerboundContainerButtonClick {
    const ID: i32 = 0x11;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        buf.write_varint(self.button_id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            container_id: buf.read_varint()?,
            button_id: buf.read_varint()?,
        })
    }
}

/// `Serverbound Container Click` (SB). Verified against the local vanilla 26.1.2 class:
/// `ServerboundContainerClickPacket(int containerId, int stateId, short slotNum,
/// byte buttonNum, ContainerInput containerInput, Int2ObjectMap<HashedStack>
/// changedSlots, HashedStack carriedItem)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundContainerClick {
    pub container_id: i32,
    pub state_id: i32,
    pub slot_num: i16,
    pub button_num: i8,
    pub container_input: ContainerInput,
    pub changed_slots: Vec<(i16, HashedStack)>,
    pub carried_item: HashedStack,
}

impl Packet for ServerboundContainerClick {
    const ID: i32 = 0x12;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.changed_slots.len() > MAX_CONTAINER_CLICK_CHANGED_SLOTS {
            return Err(CodecError::StringTooLong {
                len: self.changed_slots.len(),
                max: MAX_CONTAINER_CLICK_CHANGED_SLOTS,
            });
        }
        let mut component_hash_count = 0;
        for (_, stack) in &self.changed_slots {
            add_component_hashes_to_budget(
                &mut component_hash_count,
                stack.component_hash_count()?,
            )?;
        }
        add_component_hashes_to_budget(
            &mut component_hash_count,
            self.carried_item.component_hash_count()?,
        )?;

        buf.write_varint(self.container_id);
        buf.write_varint(self.state_id);
        buf.write_i16(self.slot_num);
        buf.write_i8(self.button_num);
        buf.write_varint(self.container_input.as_wire());
        write_count(buf, self.changed_slots.len())?;
        for (slot, stack) in &self.changed_slots {
            buf.write_i16(*slot);
            stack.encode(buf)?;
        }
        self.carried_item.encode(buf)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let container_id = buf.read_varint()?;
        let state_id = buf.read_varint()?;
        let slot_num = buf.read_i16()?;
        let button_num = buf.read_i8()?;
        let container_input = ContainerInput::from_wire(buf.read_varint()?)?;
        let changed_len = read_count(buf, MAX_CONTAINER_CLICK_CHANGED_SLOTS)?;
        require_remaining(buf, changed_len * 3 + 1)?;
        let mut changed_slots = Vec::with_capacity(changed_len);
        let mut component_hash_count = 0;
        for _ in 0..changed_len {
            changed_slots.push((
                buf.read_i16()?,
                HashedStack::decode(buf, &mut component_hash_count)?,
            ));
        }
        let carried_item = HashedStack::decode(buf, &mut component_hash_count)?;
        Ok(Self {
            container_id,
            state_id,
            slot_num,
            button_num,
            container_input,
            changed_slots,
            carried_item,
        })
    }
}

/// `Serverbound Set Carried Item` (SB). Sent when the client scrolls
/// the hotbar — `slot` ∈ `0..=8`. Per ADR 0002, verified against
/// `javap -p`: `ServerboundSetCarriedItemPacket(short slot)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundSetCarriedItem {
    pub slot: i16,
}

impl Packet for ServerboundSetCarriedItem {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // SERVERBOUND_SET_CARRIED_ITEM at game-SB index 53 = wire id 0x35.
    const ID: i32 = 0x35;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i16(self.slot);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            slot: buf.read_i16()?,
        })
    }
}

/// `Serverbound Set Creative Mode Slot` (SB). While in creative mode the
/// client reports the FULL new stack for a player-inventory slot — taking
/// an item from the creative menu, overwriting a slot, or destroying the
/// slot's stack (empty payload). A negative `slot` is the creative
/// drop/destroy action. Per ADR 0002, verified against `javap -p` of the
/// unobfuscated 26.1.2 jar:
/// `ServerboundSetCreativeModeSlotPacket(short slotNum, ItemStack
/// itemStack)` with `STREAM_CODEC = composite(ByteBufCodecs.SHORT,
/// ItemStack.OPTIONAL_UNTRUSTED_STREAM_CODEC)` (wrapped by
/// `validatedStreamCodec`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundSetCreativeModeSlot {
    pub slot: i16,
    pub item_stack: ItemStack,
}

impl Packet for ServerboundSetCreativeModeSlot {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // SERVERBOUND_SET_CREATIVE_MODE_SLOT at game-SB index 56 = wire id
    // 0x38. Cross-checked against the pinned ids of the same builder
    // lambda: SERVERBOUND_PLAYER_ACTION at index 41 = 0x29 and
    // SERVERBOUND_SET_CARRIED_ITEM at index 53 = 0x35.
    const ID: i32 = 0x38;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i16(self.slot);
        self.item_stack.encode(buf)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            slot: buf.read_i16()?,
            item_stack: ItemStack::decode(buf)?,
        })
    }
}

/// `Serverbound Place Recipe` (SB). Per local 26.1.2 `javap`:
/// `ServerboundPlaceRecipePacket(int containerId, RecipeDisplayId recipe,
/// boolean useMaxItems)`, encoded as container-id VarInt, recipe display
/// index VarInt, then bool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundPlaceRecipe {
    pub container_id: i32,
    pub recipe_display_id: i32,
    pub use_max_items: bool,
}

impl Packet for ServerboundPlaceRecipe {
    // GameProtocols registers SERVERBOUND_PLACE_RECIPE immediately before
    // PLAYER_ABILITIES / PLAYER_ACTION; with SERVERBOUND_PLAYER_ACTION pinned
    // at index 41, this packet is game-SB index 39 = wire id 0x27.
    const ID: i32 = 0x27;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        buf.write_varint(self.recipe_display_id);
        buf.write_bool(self.use_max_items);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            container_id: buf.read_varint()?,
            recipe_display_id: buf.read_varint()?,
            use_max_items: buf.read_bool()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RecipeBookTypeSettings {
    pub is_open: bool,
    pub is_filtering: bool,
}

/// Clientbound recipe-book UI settings. The local vanilla 26.1.2 codec writes
/// one open/filtering boolean pair for each of the four recipe-book types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClientboundRecipeBookSettings {
    pub crafting: RecipeBookTypeSettings,
    pub furnace: RecipeBookTypeSettings,
    pub blast_furnace: RecipeBookTypeSettings,
    pub smoker: RecipeBookTypeSettings,
}

impl Packet for ClientboundRecipeBookSettings {
    const ID: i32 = 0x4C;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        for settings in [self.crafting, self.furnace, self.blast_furnace, self.smoker] {
            buf.write_bool(settings.is_open);
            buf.write_bool(settings.is_filtering);
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let mut read_settings = || -> Result<RecipeBookTypeSettings, CodecError> {
            Ok(RecipeBookTypeSettings {
                is_open: buf.read_bool()?,
                is_filtering: buf.read_bool()?,
            })
        };
        Ok(Self {
            crafting: read_settings()?,
            furnace: read_settings()?,
            blast_furnace: read_settings()?,
            smoker: read_settings()?,
        })
    }
}

/// The slot-display variants Solaris emits in initial recipe-book entries.
/// Registry ids are pinned to the bundled 26.1.2 `minecraft:slot_display`
/// registry report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipeBookSlotDisplay {
    Empty,
    AnyFuel,
    Item { item_id: i32 },
    ItemStack(Box<ItemStack>),
    Tag(Identifier),
    Composite(Vec<RecipeBookSlotDisplay>),
}

impl RecipeBookSlotDisplay {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.encode_with_depth(buf, 0)
    }

    fn encode_with_depth<B: BufMut>(&self, buf: &mut B, depth: usize) -> Result<(), CodecError> {
        if depth >= MAX_RECIPE_BOOK_SLOT_DEPTH {
            return Err(CodecError::NotSupported(
                "recipe-book slot display nesting is too deep",
            ));
        }
        match self {
            Self::Empty => buf.write_varint(0),
            Self::AnyFuel => buf.write_varint(1),
            Self::Item { item_id } => {
                if *item_id < 0 {
                    return Err(CodecError::NotSupported(
                        "negative recipe-book item registry id",
                    ));
                }
                buf.write_varint(4);
                buf.write_varint(*item_id);
            }
            Self::ItemStack(stack) => {
                if stack.item_id > i32::MAX as u32 || stack.count <= 0 {
                    return Err(CodecError::NotSupported("invalid recipe-book item stack"));
                }
                buf.write_varint(5);
                buf.write_varint(stack.item_id as i32);
                buf.write_varint(stack.count);
                stack.encode_components(buf)?;
            }
            Self::Tag(tag) => {
                buf.write_varint(6);
                buf.write_identifier(tag)?;
            }
            Self::Composite(displays) => {
                buf.write_varint(10);
                write_bounded_count(buf, displays.len(), MAX_RECIPE_BOOK_SLOTS)?;
                for display in displays {
                    display.encode_with_depth(buf, depth + 1)?;
                }
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Self::decode_with_depth(buf, 0)
    }

    fn decode_with_depth<B: Buf>(buf: &mut B, depth: usize) -> Result<Self, CodecError> {
        if depth >= MAX_RECIPE_BOOK_SLOT_DEPTH {
            return Err(CodecError::NotSupported(
                "recipe-book slot display nesting is too deep",
            ));
        }
        match buf.read_varint()? {
            0 => Ok(Self::Empty),
            1 => Ok(Self::AnyFuel),
            4 => {
                let item_id = buf.read_varint()?;
                if item_id < 0 {
                    return Err(CodecError::NotSupported(
                        "negative recipe-book item registry id",
                    ));
                }
                Ok(Self::Item { item_id })
            }
            5 => {
                let item_id = buf.read_varint()?;
                let count = buf.read_varint()?;
                if item_id < 0 || count <= 0 {
                    return Err(CodecError::NotSupported("invalid recipe-book item stack"));
                }
                Ok(Self::ItemStack(Box::new(ItemStack::decode_components(
                    buf,
                    item_id as u32,
                    count,
                )?)))
            }
            6 => Ok(Self::Tag(buf.read_identifier()?)),
            10 => {
                let count = read_count(buf, MAX_RECIPE_BOOK_SLOTS)?;
                let mut displays = bounded_vec(count);
                for _ in 0..count {
                    displays.push(Self::decode_with_depth(buf, depth + 1)?);
                }
                Ok(Self::Composite(displays))
            }
            _ => Err(CodecError::NotSupported(
                "unsupported recipe-book slot display type",
            )),
        }
    }
}

/// Vanilla's holder-set representation used by crafting requirements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipeBookIngredient {
    Tag(Identifier),
    Items(Vec<i32>),
}

/// One entry in the recipe-property-set map carried by
/// `ClientboundUpdateRecipesPacket`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipePropertySet {
    pub key: Identifier,
    pub item_ids: Vec<i32>,
}

/// One input/output offer in vanilla's stonecutter recipe set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StonecutterRecipeEntry {
    pub input: RecipeBookIngredient,
    pub result: RecipeBookSlotDisplay,
}

/// `ClientboundUpdateRecipesPacket` from the local vanilla 26.1.2 jar.
///
/// The first field is a map of recipe-property-set keys to item registry-id
/// lists. The second is the stonecutter's single-input offer list. The codec
/// preserves both property-set membership and item-stack stonecutter displays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundUpdateRecipes {
    pub item_sets: Vec<RecipePropertySet>,
    pub stonecutter_recipes: Vec<StonecutterRecipeEntry>,
}

impl Packet for ClientboundUpdateRecipes {
    // GameProtocols' bundle packet is wire id 0. UPDATE_RECIPES is the
    // 133rd following clientbound registration, giving wire id 0x85.
    const ID: i32 = 0x85;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_bounded_count(buf, self.item_sets.len(), MAX_RECIPE_BOOK_ENTRIES)?;
        for item_set in &self.item_sets {
            buf.write_identifier(&item_set.key)?;
            write_bounded_count(
                buf,
                item_set.item_ids.len(),
                MAX_RECIPE_BOOK_INGREDIENT_ITEMS,
            )?;
            for item_id in &item_set.item_ids {
                if *item_id < 0 {
                    return Err(CodecError::NotSupported(
                        "negative recipe property-set item registry id",
                    ));
                }
                buf.write_varint(*item_id);
            }
        }
        write_bounded_count(buf, self.stonecutter_recipes.len(), MAX_RECIPE_BOOK_ENTRIES)?;
        for recipe in &self.stonecutter_recipes {
            recipe.input.encode(buf)?;
            recipe.result.encode(buf)?;
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let item_set_count = read_count(buf, MAX_RECIPE_BOOK_ENTRIES)?;
        let mut item_sets = bounded_vec(item_set_count);
        for _ in 0..item_set_count {
            let key = buf.read_identifier()?;
            let item_count = read_count(buf, MAX_RECIPE_BOOK_INGREDIENT_ITEMS)?;
            let mut item_ids = bounded_vec(item_count);
            for _ in 0..item_count {
                let item_id = buf.read_varint()?;
                if item_id < 0 {
                    return Err(CodecError::NotSupported(
                        "negative recipe property-set item registry id",
                    ));
                }
                item_ids.push(item_id);
            }
            item_sets.push(RecipePropertySet { key, item_ids });
        }

        let recipe_count = read_count(buf, MAX_RECIPE_BOOK_ENTRIES)?;
        let mut stonecutter_recipes = bounded_vec(recipe_count);
        for _ in 0..recipe_count {
            stonecutter_recipes.push(StonecutterRecipeEntry {
                input: RecipeBookIngredient::decode(buf)?,
                result: RecipeBookSlotDisplay::decode(buf)?,
            });
        }
        Ok(Self {
            item_sets,
            stonecutter_recipes,
        })
    }
}

impl RecipeBookIngredient {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        match self {
            Self::Tag(tag) => {
                buf.write_varint(0);
                buf.write_identifier(tag)?;
            }
            Self::Items(items) => {
                if items.is_empty() {
                    return Err(CodecError::NotSupported(
                        "empty direct recipe-book ingredient",
                    ));
                }
                if items.len() > MAX_RECIPE_BOOK_INGREDIENT_ITEMS {
                    return Err(CodecError::StringTooLong {
                        len: items.len(),
                        max: MAX_RECIPE_BOOK_INGREDIENT_ITEMS,
                    });
                }
                let marker =
                    i32::try_from(items.len() + 1).map_err(|_| CodecError::StringTooLong {
                        len: items.len(),
                        max: i32::MAX as usize - 1,
                    })?;
                buf.write_varint(marker);
                for item_id in items {
                    if *item_id < 0 {
                        return Err(CodecError::NotSupported(
                            "negative recipe-book item registry id",
                        ));
                    }
                    buf.write_varint(*item_id);
                }
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let marker = buf.read_varint()?;
        if marker < 0 {
            return Err(CodecError::NegativeLength(marker));
        }
        if marker == 0 {
            return Ok(Self::Tag(buf.read_identifier()?));
        }
        let count = marker as usize - 1;
        if count == 0 {
            return Err(CodecError::NotSupported(
                "empty direct recipe-book ingredient",
            ));
        }
        if count > MAX_RECIPE_BOOK_INGREDIENT_ITEMS {
            return Err(CodecError::StringTooLong {
                len: count,
                max: MAX_RECIPE_BOOK_INGREDIENT_ITEMS,
            });
        }
        let mut items = bounded_vec(count);
        for _ in 0..count {
            let item_id = buf.read_varint()?;
            if item_id < 0 {
                return Err(CodecError::NotSupported(
                    "negative recipe-book item registry id",
                ));
            }
            items.push(item_id);
        }
        Ok(Self::Items(items))
    }
}

/// Recipe display variants currently backed by Solaris' recipe executor.
#[derive(Debug, Clone, PartialEq)]
pub enum RecipeBookDisplay {
    Shapeless {
        ingredients: Vec<RecipeBookSlotDisplay>,
        result: RecipeBookSlotDisplay,
        crafting_station: RecipeBookSlotDisplay,
    },
    Shaped {
        width: i32,
        height: i32,
        ingredients: Vec<RecipeBookSlotDisplay>,
        result: RecipeBookSlotDisplay,
        crafting_station: RecipeBookSlotDisplay,
    },
    Furnace {
        ingredient: RecipeBookSlotDisplay,
        fuel: RecipeBookSlotDisplay,
        result: RecipeBookSlotDisplay,
        crafting_station: RecipeBookSlotDisplay,
        duration: i32,
        experience: f32,
    },
}

impl RecipeBookDisplay {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        match self {
            Self::Shapeless {
                ingredients,
                result,
                crafting_station,
            } => {
                buf.write_varint(0);
                write_bounded_count(buf, ingredients.len(), MAX_RECIPE_BOOK_SLOTS)?;
                for ingredient in ingredients {
                    ingredient.encode(buf)?;
                }
                result.encode(buf)?;
                crafting_station.encode(buf)?;
            }
            Self::Shaped {
                width,
                height,
                ingredients,
                result,
                crafting_station,
            } => {
                let expected = recipe_book_shape_size(*width, *height)?;
                if ingredients.len() != expected {
                    return Err(CodecError::NotSupported(
                        "recipe-book shaped ingredient count does not match dimensions",
                    ));
                }
                buf.write_varint(1);
                buf.write_varint(*width);
                buf.write_varint(*height);
                write_bounded_count(buf, ingredients.len(), MAX_RECIPE_BOOK_SLOTS)?;
                for ingredient in ingredients {
                    ingredient.encode(buf)?;
                }
                result.encode(buf)?;
                crafting_station.encode(buf)?;
            }
            Self::Furnace {
                ingredient,
                fuel,
                result,
                crafting_station,
                duration,
                experience,
            } => {
                if *duration < 0 {
                    return Err(CodecError::NotSupported(
                        "negative recipe-book cooking duration",
                    ));
                }
                buf.write_varint(2);
                ingredient.encode(buf)?;
                fuel.encode(buf)?;
                result.encode(buf)?;
                crafting_station.encode(buf)?;
                buf.write_varint(*duration);
                buf.write_f32(*experience);
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        match buf.read_varint()? {
            0 => {
                let count = read_count(buf, MAX_RECIPE_BOOK_SLOTS)?;
                let mut ingredients = bounded_vec(count);
                for _ in 0..count {
                    ingredients.push(RecipeBookSlotDisplay::decode(buf)?);
                }
                Ok(Self::Shapeless {
                    ingredients,
                    result: RecipeBookSlotDisplay::decode(buf)?,
                    crafting_station: RecipeBookSlotDisplay::decode(buf)?,
                })
            }
            1 => {
                let width = buf.read_varint()?;
                let height = buf.read_varint()?;
                let expected = recipe_book_shape_size(width, height)?;
                let count = read_count(buf, MAX_RECIPE_BOOK_SLOTS)?;
                if count != expected {
                    return Err(CodecError::NotSupported(
                        "recipe-book shaped ingredient count does not match dimensions",
                    ));
                }
                let mut ingredients = bounded_vec(count);
                for _ in 0..count {
                    ingredients.push(RecipeBookSlotDisplay::decode(buf)?);
                }
                Ok(Self::Shaped {
                    width,
                    height,
                    ingredients,
                    result: RecipeBookSlotDisplay::decode(buf)?,
                    crafting_station: RecipeBookSlotDisplay::decode(buf)?,
                })
            }
            2 => {
                let ingredient = RecipeBookSlotDisplay::decode(buf)?;
                let fuel = RecipeBookSlotDisplay::decode(buf)?;
                let result = RecipeBookSlotDisplay::decode(buf)?;
                let crafting_station = RecipeBookSlotDisplay::decode(buf)?;
                let duration = buf.read_varint()?;
                if duration < 0 {
                    return Err(CodecError::NotSupported(
                        "negative recipe-book cooking duration",
                    ));
                }
                Ok(Self::Furnace {
                    ingredient,
                    fuel,
                    result,
                    crafting_station,
                    duration,
                    experience: buf.read_f32()?,
                })
            }
            _ => Err(CodecError::NotSupported(
                "unsupported recipe-book display type",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecipeBookEntry {
    pub display_id: i32,
    pub display: RecipeBookDisplay,
    pub group: Option<i32>,
    pub category_id: i32,
    pub crafting_requirements: Option<Vec<RecipeBookIngredient>>,
    pub flags: u8,
}

impl RecipeBookEntry {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.display_id < 0 || self.category_id < 0 {
            return Err(CodecError::NotSupported(
                "negative recipe-book display or category id",
            ));
        }
        buf.write_varint(self.display_id);
        self.display.encode(buf)?;
        match self.group {
            None => buf.write_varint(0),
            Some(group) if (0..i32::MAX).contains(&group) => buf.write_varint(group + 1),
            Some(_) => {
                return Err(CodecError::NotSupported("invalid recipe-book group id"));
            }
        }
        buf.write_varint(self.category_id);
        match &self.crafting_requirements {
            None => buf.write_bool(false),
            Some(requirements) => {
                buf.write_bool(true);
                write_bounded_count(buf, requirements.len(), MAX_RECIPE_BOOK_REQUIREMENTS)?;
                for requirement in requirements {
                    requirement.encode(buf)?;
                }
            }
        }
        buf.write_u8(self.flags);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let display_id = buf.read_varint()?;
        if display_id < 0 {
            return Err(CodecError::NotSupported("negative recipe-book display id"));
        }
        let display = RecipeBookDisplay::decode(buf)?;
        let group_marker = buf.read_varint()?;
        if group_marker < 0 {
            return Err(CodecError::NegativeLength(group_marker));
        }
        let group = (group_marker != 0).then_some(group_marker - 1);
        let category_id = buf.read_varint()?;
        if category_id < 0 {
            return Err(CodecError::NotSupported("negative recipe-book category id"));
        }
        let crafting_requirements = if buf.read_bool()? {
            let count = read_count(buf, MAX_RECIPE_BOOK_REQUIREMENTS)?;
            let mut requirements = bounded_vec(count);
            for _ in 0..count {
                requirements.push(RecipeBookIngredient::decode(buf)?);
            }
            Some(requirements)
        } else {
            None
        };
        Ok(Self {
            display_id,
            display,
            group,
            category_id,
            crafting_requirements,
            flags: buf.read_u8()?,
        })
    }
}

/// `ClientboundRecipeBookAddPacket` from the local vanilla 26.1.2 jar.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundRecipeBookAdd {
    pub entries: Vec<RecipeBookEntry>,
    pub replace: bool,
}

impl Packet for ClientboundRecipeBookAdd {
    const ID: i32 = 0x4A;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_bounded_count(buf, self.entries.len(), MAX_RECIPE_BOOK_ENTRIES)?;
        for entry in &self.entries {
            entry.encode(buf)?;
        }
        buf.write_bool(self.replace);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let count = read_count(buf, MAX_RECIPE_BOOK_ENTRIES)?;
        let mut entries = bounded_vec(count);
        for _ in 0..count {
            entries.push(RecipeBookEntry::decode(buf)?);
        }
        Ok(Self {
            entries,
            replace: buf.read_bool()?,
        })
    }
}

/// `Serverbound Container Close` (SB). Verified against the local vanilla 26.1.2 class:
/// `ServerboundContainerClosePacket` carries one `readContainerId` VarInt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundContainerClose {
    pub container_id: i32,
}

impl Packet for ServerboundContainerClose {
    const ID: i32 = 0x13;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.container_id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            container_id: buf.read_varint()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipeBookType {
    Crafting,
    Furnace,
    BlastFurnace,
    Smoker,
}

impl RecipeBookType {
    const fn as_wire(self) -> i32 {
        match self {
            Self::Crafting => 0,
            Self::Furnace => 1,
            Self::BlastFurnace => 2,
            Self::Smoker => 3,
        }
    }

    fn from_wire(value: i32) -> Result<Self, CodecError> {
        Ok(match value {
            0 => Self::Crafting,
            1 => Self::Furnace,
            2 => Self::BlastFurnace,
            3 => Self::Smoker,
            other => {
                return Err(CodecError::StringTooLong {
                    len: other as usize,
                    max: 3,
                });
            }
        })
    }
}

/// `Serverbound Recipe Book Change Settings` (SB). Verified against the local
/// vanilla 26.1.2 class: `RecipeBookType` enum ordinal, then two booleans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundRecipeBookChangeSettings {
    pub book_type: RecipeBookType,
    pub is_open: bool,
    pub is_filtering: bool,
}

impl Packet for ServerboundRecipeBookChangeSettings {
    const ID: i32 = 0x2E;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.book_type.as_wire());
        buf.write_bool(self.is_open);
        buf.write_bool(self.is_filtering);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            book_type: RecipeBookType::from_wire(buf.read_varint()?)?,
            is_open: buf.read_bool()?,
            is_filtering: buf.read_bool()?,
        })
    }
}

/// `Serverbound Recipe Book Seen Recipe` (SB). Verified against the local
/// vanilla 26.1.2 class: one `RecipeDisplayId` VarInt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundRecipeBookSeenRecipe {
    pub recipe_display_id: i32,
}

impl Packet for ServerboundRecipeBookSeenRecipe {
    const ID: i32 = 0x2F;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.recipe_display_id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            recipe_display_id: buf.read_varint()?,
        })
    }
}
