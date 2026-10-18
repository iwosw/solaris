//! Player inventory, menus and crafting (ADR 0013).
//!
//! Pure logic over owned state. The tick's merge step applies one click at a
//! time, in its fixed order, against the accumulated state; this module turns a
//! click into new slot values, a new cursor, and items to drop. Click rules
//! follow vanilla `AbstractContainerMenu.doClick` so that the client's own
//! prediction of a click matches the server result.

pub mod anvil;
mod click;
#[cfg(test)]
mod click_tests;
mod craft_stations;
#[cfg(test)]
mod craft_stations_tests;
mod crafting;
pub mod enchanting;
pub mod furnace;
#[cfg(test)]
mod furnace_tests;
pub mod grindstone;
mod recipe_book;
#[cfg(test)]
mod recipe_book_tests;
mod stations;

pub use click::ClickOutcome;

use mc_data::ItemStack;
use mc_data::item_components::ItemFactsTable;
use mc_data::item_semantics_26_1_2::max_stack_for_stack;
use mc_data::items::ItemRegistry;
use mc_data::recipes::{CookingKind, Recipe};
use mc_data::tags::TagsData;

/// Vanilla default container slot limit (`Container.getMaxStackSize`).
const CONTAINER_MAX_STACK: i32 = 99;

/// Player inventory size: 0-8 hotbar, 9-35 main, 36-39 armor (feet, legs,
/// chest, head), 40 off hand.
pub const PLAYER_SLOTS: usize = 41;
pub const OFF_HAND: usize = 40;

/// Immutable item data the rules read; built once at startup.
#[derive(Clone, Copy)]
pub struct ItemRules<'a> {
    pub items: &'a ItemRegistry,
    pub facts: &'a ItemFactsTable,
    pub tags: &'a TagsData,
    pub recipes: &'a [Recipe],
}

impl ItemRules<'_> {
    #[must_use]
    pub fn max_stack(&self, stack: &ItemStack) -> i32 {
        max_stack_for_stack(self.facts, self.items, stack)
    }

    /// Vanilla `BeaconMenu.PaymentSlot.mayPlace`: the
    /// `minecraft:beacon_payment_items` tag (iron/gold ingot, emerald,
    /// diamond, netherite ingot). Plugin resources never pay with their wire
    /// carrier, matching the station guards.
    #[must_use]
    pub fn is_beacon_payment(&self, stack: &ItemStack) -> bool {
        if stack.is_empty() {
            return true;
        }
        let Some(name) = anvil::logical_item(self, stack) else {
            return false;
        };
        if self.items.name_of(stack.item_id) != Some(name) {
            return false;
        }
        self.tags.contains_raw_id(
            "minecraft:item",
            "beacon_payment_items",
            stack.item_id as i32,
        ) || matches!(
            name.as_str(),
            "minecraft:iron_ingot"
                | "minecraft:gold_ingot"
                | "minecraft:emerald"
                | "minecraft:diamond"
                | "minecraft:netherite_ingot"
        )
    }
    fn equipment(&self, stack: &ItemStack) -> Option<Equipment> {
        match self
            .facts
            .facts_for_stack(stack, self.items)?
            .equippable_slot
            .as_deref()?
        {
            "feet" => Some(Equipment::Feet),
            "legs" => Some(Equipment::Legs),
            "chest" => Some(Equipment::Chest),
            "head" => Some(Equipment::Head),
            "offhand" => Some(Equipment::OffHand),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Equipment {
    Feet,
    Legs,
    Chest,
    Head,
    OffHand,
}

impl Equipment {
    /// Player inventory index of this equipment slot.
    fn inventory_index(self) -> usize {
        match self {
            Self::Feet => 36,
            Self::Legs => 37,
            Self::Chest => 38,
            Self::Head => 39,
            Self::OffHand => OFF_HAND,
        }
    }
}

/// A player's own inventory.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerInventory {
    pub slots: Vec<ItemStack>,
    /// Selected hotbar slot, 0-8.
    pub selected: u8,
}

impl Default for PlayerInventory {
    fn default() -> Self {
        Self {
            slots: vec![ItemStack::EMPTY; PLAYER_SLOTS],
            selected: 0,
        }
    }
}

impl PlayerInventory {
    /// Vanilla `Inventory.add`: top up a matching stack (selected slot, off
    /// hand, then 0-35), else fill the first empty slot of 0-35; repeat while
    /// it makes progress. What does not fit stays in `stack`.
    pub fn add(&mut self, rules: &ItemRules<'_>, stack: &mut ItemStack) {
        while !stack.is_empty() {
            let before = stack.count;
            let max = rules.max_stack(stack).min(CONTAINER_MAX_STACK);
            let target = self
                .slot_with_space(stack, max)
                .or_else(|| (0..36).find(|&index| self.slots[index].is_empty()));
            let Some(index) = target else { return };
            let slot = &mut self.slots[index];
            if slot.is_empty() {
                let moved = stack.count.min(max);
                *slot = stack.clone();
                slot.count = moved;
                stack.count -= moved;
            } else {
                let moved = stack.count.min(max - slot.count);
                slot.count += moved;
                stack.count -= moved;
            }
            if stack.count == before {
                return;
            }
        }
        *stack = ItemStack::EMPTY;
    }

    fn slot_with_space(&self, stack: &ItemStack, max: i32) -> Option<usize> {
        let fits = |index: usize| {
            let slot = &self.slots[index];
            mc_data::inventory_semantics_26_1_2::can_stack(slot, stack) && slot.count < max
        };
        let selected = usize::from(self.selected.min(8));
        if fits(selected) {
            return Some(selected);
        }
        if fits(OFF_HAND) {
            return Some(OFF_HAND);
        }
        (0..36).find(|&index| fits(index))
    }
}

/// Menu layouts the core opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    /// Window 0: 2x2 crafting, armor, inventory, off hand.
    Player,
    CraftingTable,
    EnchantingTable,
    Anvil,
    Grindstone,
    Smithing,
    Loom,
    Stonecutter,
    Hopper,
    Dispenser,
    Crafter,
    ShulkerBox,
    BrewingStand,
    /// Villager merchant: two payment slots and one derived result.
    Merchant,
    /// Beacon: one transient payment slot; effect selection is tick-owned.
    Beacon,
    /// Generic `rows` x 9 container (chest, barrel, double chest).
    Chest {
        rows: u8,
    },
    /// Furnace, smoker or blast furnace: 0 input, 1 fuel, 2 output.
    Furnace(CookingKind),
}

/// Where a menu slot index points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotRef {
    Result,
    Grid(usize),
    Inventory(usize),
    Container(usize),
}

impl MenuKind {
    /// Protocol menu type id (`minecraft:menu` registry); `None` for window 0.
    #[must_use]
    pub fn menu_type(self) -> Option<u32> {
        match self {
            Self::Player => None,
            Self::CraftingTable => Some(12),
            Self::Chest { rows } => Some(u32::from(rows.clamp(1, 6)) - 1),
            Self::Furnace(CookingKind::Furnace) => Some(14),
            Self::Furnace(CookingKind::Smoker) => Some(22),
            Self::Furnace(CookingKind::BlastFurnace) => Some(10),
            Self::EnchantingTable => Some(13),
            Self::Anvil => Some(8),
            // `javap MenuType`: beacon registers right after anvil.
            Self::Beacon => Some(9),
            Self::Grindstone => Some(15),
            Self::Merchant => Some(19),
            Self::Smithing => Some(21),
            Self::Loom => Some(18),
            Self::Stonecutter => Some(24),
            Self::Hopper => Some(16),
            Self::Dispenser => Some(6),
            Self::Crafter => Some(7),
            Self::ShulkerBox => Some(20),
            Self::BrewingStand => Some(11),
        }
    }

    #[must_use]
    pub fn slot_count(self) -> usize {
        match self {
            Self::Player => 46,
            Self::CraftingTable => 46,
            Self::Chest { rows } => usize::from(rows) * 9 + 36,
            Self::ShulkerBox => 63,
            Self::Furnace(_) => 39,
            Self::EnchantingTable => 38,
            Self::Anvil | Self::Grindstone | Self::Merchant => 39,
            // `javap BeaconMenu`: SLOT_COUNT 1, player tail 1..37.
            Self::Beacon => 37,
            Self::Smithing | Self::Loom => 40,
            Self::Stonecutter => 38,
            Self::Hopper | Self::BrewingStand => 41,
            Self::Dispenser => 45,
            Self::Crafter => 46,
        }
    }

    /// Slots of the opened block's storage.
    #[must_use]
    pub fn container_size(self) -> usize {
        match self {
            Self::Player
            | Self::CraftingTable
            | Self::EnchantingTable
            | Self::Anvil
            | Self::Grindstone
            | Self::Merchant
            | Self::Smithing
            | Self::Loom
            | Self::Stonecutter
            | Self::Beacon => 0,
            Self::Chest { rows } => usize::from(rows) * 9,
            Self::ShulkerBox => 27,
            Self::Furnace(_) => 3,
            Self::Hopper | Self::BrewingStand => 5,
            Self::Dispenser | Self::Crafter => 9,
        }
    }

    fn grid_size(self) -> usize {
        match self {
            Self::Player => 4,
            Self::CraftingTable => 9,
            Self::EnchantingTable | Self::Anvil | Self::Grindstone | Self::Merchant => 2,
            Self::Smithing | Self::Loom => 3,
            Self::Stonecutter | Self::Beacon => 1,
            Self::Chest { .. }
            | Self::Furnace(_)
            | Self::Hopper
            | Self::Dispenser
            | Self::Crafter
            | Self::ShulkerBox
            | Self::BrewingStand => 0,
        }
    }

    #[must_use]
    pub fn slot(self, index: usize) -> Option<SlotRef> {
        let player_tail = |index: usize| match index {
            0..27 => Some(SlotRef::Inventory(index + 9)),
            27..36 => Some(SlotRef::Inventory(index - 27)),
            _ => None,
        };
        match self {
            Self::Player => match index {
                0 => Some(SlotRef::Result),
                1..=4 => Some(SlotRef::Grid(index - 1)),
                5..=8 => Some(SlotRef::Inventory(44 - index)),
                9..=44 => player_tail(index - 9),
                45 => Some(SlotRef::Inventory(OFF_HAND)),
                _ => None,
            },
            Self::CraftingTable => match index {
                0 => Some(SlotRef::Result),
                1..=9 => Some(SlotRef::Grid(index - 1)),
                10..=45 => player_tail(index - 10),
                _ => None,
            },
            Self::EnchantingTable => match index {
                0..=1 => Some(SlotRef::Grid(index)),
                2..=37 => player_tail(index - 2),
                _ => None,
            },
            Self::Anvil | Self::Grindstone | Self::Merchant => match index {
                0..=1 => Some(SlotRef::Grid(index)),
                2 => Some(SlotRef::Result),
                3..=38 => player_tail(index - 3),
                _ => None,
            },
            Self::Smithing | Self::Loom => match index {
                0..=2 => Some(SlotRef::Grid(index)),
                3 => Some(SlotRef::Result),
                4..=39 => player_tail(index - 4),
                _ => None,
            },
            Self::Stonecutter => match index {
                0 => Some(SlotRef::Grid(0)),
                1 => Some(SlotRef::Result),
                2..=37 => player_tail(index - 2),
                _ => None,
            },
            // `javap BeaconMenu`: payment slot 0, player tail 1..37.
            Self::Beacon => match index {
                0 => Some(SlotRef::Grid(0)),
                1..=36 => player_tail(index - 1),
                _ => None,
            },
            Self::Crafter => match index {
                0..=8 => Some(SlotRef::Container(index)),
                9..=44 => player_tail(index - 9),
                45 => Some(SlotRef::Result),
                _ => None,
            },
            Self::Chest { .. }
            | Self::Furnace(_)
            | Self::Hopper
            | Self::Dispenser
            | Self::ShulkerBox
            | Self::BrewingStand => {
                let container = self.container_size();
                if index < container {
                    Some(SlotRef::Container(index))
                } else {
                    player_tail(index - container)
                }
            }
        }
    }
}

/// One open menu of a player. Window 0 exists for the player's lifetime.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub window: u8,
    pub kind: MenuKind,
    pub state_id: i32,
    pub carried: ItemStack,
    /// Crafting grid owned by this menu (2x2 or 3x3); emptied back into the
    /// inventory on close.
    pub grid: Vec<ItemStack>,
    pub result: ItemStack,
    pub(crate) crafter_disabled: u16,
    quick_craft: QuickCraft,
    pub station: stations::StationState,
    /// Merchant identity/selection are transient; payment slots are returned
    /// through the ordinary close/disconnect path.
    pub merchant: Option<MerchantMenu>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MerchantMenu {
    pub entity: crate::boundary::EntityId,
    pub selected: usize,
    pub last_restock: i64,
    pub reputation: i32,
    pub hero: Option<u8>,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct QuickCraft {
    status: u8,
    kind: u8,
    slots: Vec<usize>,
}

impl Menu {
    #[must_use]
    pub fn new(window: u8, kind: MenuKind) -> Self {
        Self {
            window,
            kind,
            state_id: 0,
            carried: ItemStack::EMPTY,
            grid: vec![ItemStack::EMPTY; kind.grid_size()],
            result: ItemStack::EMPTY,
            crafter_disabled: 0,
            quick_craft: QuickCraft::default(),
            station: stations::StationState::default(),
            merchant: None,
        }
    }

    /// Full contents in menu slot order, for `MenuContents`.
    #[must_use]
    pub fn contents(&self, inventory: &PlayerInventory, container: &[ItemStack]) -> Vec<ItemStack> {
        (0..self.kind.slot_count())
            .map(|index| match self.kind.slot(index) {
                Some(SlotRef::Result) => self.result.clone(),
                Some(SlotRef::Grid(i)) => self.grid[i].clone(),
                Some(SlotRef::Inventory(i)) => inventory.slots[i].clone(),
                Some(SlotRef::Container(i)) => {
                    container.get(i).cloned().unwrap_or(ItemStack::EMPTY)
                }
                None => ItemStack::EMPTY,
            })
            .collect()
    }

    /// Vanilla `removed`: the cursor and the crafting grid go back into the
    /// inventory; whatever does not fit is returned to be dropped.
    pub fn close(
        &mut self,
        rules: &ItemRules<'_>,
        inventory: &mut PlayerInventory,
    ) -> Vec<ItemStack> {
        let mut dropped = Vec::new();
        let returning = std::iter::once(std::mem::take(&mut self.carried))
            .chain(self.grid.iter_mut().map(std::mem::take));
        for mut stack in returning.collect::<Vec<_>>() {
            inventory.add(rules, &mut stack);
            if !stack.is_empty() {
                dropped.push(stack);
            }
        }
        self.carried = ItemStack::EMPTY;
        self.grid.fill(ItemStack::EMPTY);
        self.result = ItemStack::EMPTY;
        self.quick_craft = QuickCraft::default();
        dropped
    }
}
