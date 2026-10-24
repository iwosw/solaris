//! Vanilla `AbstractContainerMenu.doClick` over owned menu state.

use mc_data::ItemStack;
use mc_data::inventory_semantics_26_1_2::{can_stack, canonicalize_empty, take_from_stack};

use super::crafting::{consume_craft, grid_result};
use super::furnace;
use super::{
    CONTAINER_MAX_STACK, Equipment, ItemRules, Menu, MenuKind, PlayerInventory, QuickCraft, SlotRef,
};
use crate::boundary::ClickMode;

/// Slot id the client uses for a click outside the window.
const OUTSIDE: i16 = -999;

/// Result of one click, ready to become `PlayerChange` values and drops.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClickOutcome {
    /// Menu slots whose authoritative value changed, in slot order.
    pub changed_slots: Vec<(usize, ItemStack)>,
    /// New cursor stack when it changed.
    pub cursor: Option<ItemStack>,
    /// Stacks to spawn as item entities at the player.
    pub dropped: Vec<ItemStack>,
    /// One `(item_id, count)` per vanilla tracked drop: cursor outside, Throw,
    /// or displaced hotbar-swap overflow; excludes recipe remainders/overflow.
    pub manual_drops: Vec<(u32, i32)>,
    /// Accepted outputs credited to vanilla `ITEM_CRAFTED`, including furnace
    /// output. Repeated extractions within one click always keep the same item.
    pub crafted_output: Option<(u32, i32)>,
    /// The client clicked with a stale `state_id`: send full `MenuContents`.
    pub resync: bool,
    /// A container slot (block entity storage) changed.
    pub container_changed: bool,
    /// Furnace menus: items left the output slot (award experience with
    /// `FurnaceState::take_experience`).
    pub furnace_output_taken: bool,
    /// Furnace menus: input slot `(before, after)` when it changed (feed
    /// `FurnaceState::input_replaced`).
    pub furnace_input_change: Option<(ItemStack, ItemStack)>,
    /// One anvil/grindstone output was extracted; player owns input/XP debit.
    pub station_output_taken: bool,
    /// Actual ResultSlot takes, with pre-debit ingredients for recipe criteria.
    pub crafted_recipes: Vec<(String, Vec<ItemStack>)>,
    pub brewed_potions: Vec<String>,
    /// Outputs whose actual craft inputs were consumed by this accepted click.
    pub(crate) crafted: Vec<(u32, i32, MenuKind)>,
}

impl Menu {
    /// Applies one click from the client. `container` is the open block's
    /// storage (empty for menus without one).
    #[allow(clippy::too_many_arguments)]
    pub fn click(
        &mut self,
        rules: &ItemRules<'_>,
        inventory: &mut PlayerInventory,
        container: &mut [ItemStack],
        client_state_id: i32,
        slot: i16,
        button: i8,
        mode: ClickMode,
        creative: bool,
    ) -> ClickOutcome {
        if self.kind.is_craft_station() {
            self.result =
                super::craft_stations::refresh(self.kind, rules, &self.grid, &mut self.station);
        }
        let before = self.contents(inventory, container);
        let carried_before = self.carried.clone();
        let mut dropped = Vec::new();
        let mut manual_drops = Vec::new();
        let mut station_output_taken = false;
        let mut crafted_output = None;
        let mut crafted_recipes = Vec::new();
        let mut crafted = Vec::new();
        let result_allowed = self.station.result_allowed;
        // A merchant result is only a projection. Its owner must debit both
        // payments and increment offer uses before publishing the output.
        let merchant_result =
            (self.kind == MenuKind::Merchant).then(|| std::mem::take(&mut self.result));
        {
            let mut click = Click {
                rules,
                kind: self.kind,
                crafter_disabled: self.crafter_disabled,
                grid: &mut self.grid,
                result: &mut self.result,
                carried: &mut self.carried,
                quick_craft: &mut self.quick_craft,
                inventory,
                container,
                dropped: &mut dropped,
                manual_drops: &mut manual_drops,
                crafted_output: &mut crafted_output,
                creative,
                station_output_taken: &mut station_output_taken,
                crafted_recipes: &mut crafted_recipes,
                crafted: &mut crafted,
                station: &mut self.station,
            };
            if result_allowed
                || slot != 2
                || !matches!(self.kind, MenuKind::Anvil | MenuKind::Grindstone)
            {
                click.run(slot, button, mode);
            }
            click.refresh_result();
        }
        if let Some(result) = merchant_result {
            self.result = result;
        }
        let resync = client_state_id != self.state_id;
        let mut outcome = self.outcome(
            &before,
            &carried_before,
            dropped,
            resync,
            inventory,
            container,
        );
        outcome.station_output_taken = station_output_taken;
        outcome.crafted_output = crafted_output;
        outcome.manual_drops = manual_drops;
        outcome.crafted_recipes = crafted_recipes;
        outcome.crafted = crafted;
        outcome
    }

    /// Diffs the menu against `before`, bumps the state id when anything
    /// changed, and reports furnace side effects.
    pub(super) fn outcome(
        &mut self,
        before: &[ItemStack],
        carried_before: &ItemStack,
        dropped: Vec<ItemStack>,
        resync: bool,
        inventory: &PlayerInventory,
        container: &[ItemStack],
    ) -> ClickOutcome {
        let after = self.contents(inventory, container);
        let changed_slots: Vec<_> = before
            .iter()
            .zip(after)
            .enumerate()
            .filter(|(_, (old, new))| *old != new)
            .map(|(index, (_, new))| (index, new))
            .collect();
        let cursor = (self.carried != *carried_before).then(|| self.carried.clone());
        if !changed_slots.is_empty() || cursor.is_some() {
            self.state_id = (self.state_id + 1) & 0x7FFF;
        }
        let container_changed = changed_slots
            .iter()
            .any(|(index, _)| matches!(self.kind.slot(*index), Some(SlotRef::Container(_))));
        let furnace = matches!(self.kind, MenuKind::Furnace(_));
        let slot_change = |slot: usize| {
            changed_slots
                .iter()
                .find(|(index, _)| *index == slot)
                .map(|(_, after)| (before[slot].clone(), after.clone()))
        };
        let furnace_output_taken = furnace
            && slot_change(furnace::OUTPUT)
                .is_some_and(|(old, new)| new.is_empty() || new.count < old.count);
        let furnace_input_change = furnace.then(|| slot_change(furnace::INPUT)).flatten();
        let brewed_potions = if self.kind == MenuKind::BrewingStand {
            (0..3)
                .filter_map(|slot| {
                    slot_change(slot).and_then(|(before, after)| {
                        (!before.is_empty()
                            && (after.item_id != before.item_id || after.count < before.count))
                            .then(|| {
                                before
                                    .extended()
                                    .potion
                                    .as_ref()
                                    .map(|potion| potion.as_str().to_owned())
                            })
                            .flatten()
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        ClickOutcome {
            changed_slots,
            cursor,
            dropped,
            manual_drops: Vec::new(),
            crafted_output: None,
            resync,
            container_changed,
            furnace_output_taken,
            furnace_input_change,
            station_output_taken: false,
            crafted_recipes: Vec::new(),
            brewed_potions,
            crafted: Vec::new(),
        }
    }
}

struct Click<'a, 'r> {
    rules: &'a ItemRules<'r>,
    kind: MenuKind,
    crafter_disabled: u16,
    grid: &'a mut Vec<ItemStack>,
    result: &'a mut ItemStack,
    carried: &'a mut ItemStack,
    quick_craft: &'a mut QuickCraft,
    inventory: &'a mut PlayerInventory,
    container: &'a mut [ItemStack],
    dropped: &'a mut Vec<ItemStack>,
    manual_drops: &'a mut Vec<(u32, i32)>,
    crafted_output: &'a mut Option<(u32, i32)>,
    creative: bool,
    station_output_taken: &'a mut bool,
    crafted: &'a mut Vec<(u32, i32, MenuKind)>,
    station: &'a mut super::stations::StationState,
    crafted_recipes: &'a mut Vec<(String, Vec<ItemStack>)>,
}

impl Click<'_, '_> {
    fn run(&mut self, slot: i16, button: i8, mode: ClickMode) {
        if mode == ClickMode::QuickCraft {
            self.quick_craft_click(slot, button);
            return;
        }
        if self.quick_craft.status != 0 {
            self.reset_quick_craft();
            return;
        }
        match mode {
            ClickMode::Pickup | ClickMode::QuickMove if button == 0 || button == 1 => {
                if slot == OUTSIDE {
                    self.drop_carried(button == 0);
                } else if let Some(index) = self.index(slot) {
                    if mode == ClickMode::QuickMove {
                        self.quick_move_click(index);
                    } else {
                        self.pickup(index, button == 0);
                    }
                }
            }
            ClickMode::Swap if (0..=8).contains(&button) || button == 40 => {
                if let Some(index) = self.index(slot) {
                    self.swap(index, usize::from(button.unsigned_abs()));
                }
            }
            ClickMode::Clone => {
                if let Some(index) = self.index(slot)
                    && self.creative
                    && self.carried.is_empty()
                    && !self.get(index).is_empty()
                {
                    let mut copy = self.get(index).clone();
                    copy.count = self.rules.max_stack(&copy);
                    *self.carried = copy;
                }
            }
            ClickMode::Throw if self.carried.is_empty() && (button == 0 || button == 1) => {
                if let Some(index) = self.index(slot) {
                    self.throw(index, button == 1);
                }
            }
            ClickMode::PickupAll => {
                if let Some(index) = self.index(slot) {
                    self.pickup_all(index, button == 0);
                }
            }
            _ => {}
        }
    }

    // ---- slot access -----------------------------------------------------

    fn index(&self, slot: i16) -> Option<usize> {
        let index = usize::try_from(slot).ok()?;
        (index < self.kind.slot_count()).then_some(index)
    }

    fn slot_ref(&self, index: usize) -> SlotRef {
        self.kind
            .slot(index)
            .expect("menu indices are validated before access")
    }

    fn get(&self, index: usize) -> &ItemStack {
        match self.slot_ref(index) {
            SlotRef::Result => self.result,
            SlotRef::Grid(i) => &self.grid[i],
            SlotRef::Inventory(i) => &self.inventory.slots[i],
            SlotRef::Container(i) => &self.container[i],
        }
    }

    fn get_mut(&mut self, index: usize) -> &mut ItemStack {
        match self.slot_ref(index) {
            SlotRef::Result => self.result,
            SlotRef::Grid(i) => &mut self.grid[i],
            SlotRef::Inventory(i) => &mut self.inventory.slots[i],
            SlotRef::Container(i) => &mut self.container[i],
        }
    }

    fn set(&mut self, index: usize, mut stack: ItemStack) {
        canonicalize_empty(&mut stack);
        *self.get_mut(index) = stack;
    }

    fn is_result(&self, index: usize) -> bool {
        self.slot_ref(index) == SlotRef::Result
    }

    fn armor_slot(&self, index: usize) -> Option<Equipment> {
        match self.slot_ref(index) {
            SlotRef::Inventory(36) => Some(Equipment::Feet),
            SlotRef::Inventory(37) => Some(Equipment::Legs),
            SlotRef::Inventory(38) => Some(Equipment::Chest),
            SlotRef::Inventory(39) => Some(Equipment::Head),
            _ => None,
        }
    }
    fn may_take(&self, index: usize) -> bool {
        if self.kind == MenuKind::Crafter && self.is_result(index) {
            return false;
        }
        self.creative
            || self.armor_slot(index).is_none()
            || crate::enchantments::level(self.get(index), "binding_curse") == 0
    }

    /// Furnace storage slot for `index`, with the menu's cooking kind.
    fn furnace_slot(&self, index: usize) -> Option<(mc_data::recipes::CookingKind, usize)> {
        match (self.kind, self.slot_ref(index)) {
            (MenuKind::Furnace(kind), SlotRef::Container(slot)) => Some((kind, slot)),
            _ => None,
        }
    }

    fn may_place(&self, index: usize, stack: &ItemStack) -> bool {
        if self.is_result(index) {
            return false;
        }
        match (self.kind, self.slot_ref(index)) {
            (MenuKind::Lectern, _) => return false,
            (MenuKind::Mount { saddle, armor, .. }, SlotRef::Container(slot @ 0..=1)) => {
                let name = self
                    .rules
                    .items
                    .name_of(stack.item_id)
                    .map(|id| id.path())
                    .unwrap_or("");
                return if slot == 0 {
                    saddle && name == "saddle"
                } else {
                    match armor {
                        super::MountArmor::None => false,
                        super::MountArmor::Horse => name.ends_with("_horse_armor"),
                        super::MountArmor::Llama => name.ends_with("_carpet"),
                    }
                };
            }
            (MenuKind::Cartography, SlotRef::Grid(slot)) => {
                return crate::maps::cartography::allowed(self.rules, slot, stack);
            }
            (MenuKind::Crafter, SlotRef::Container(slot)) => {
                return self.crafter_disabled & (1 << slot) == 0;
            }
            (MenuKind::ShulkerBox, SlotRef::Container(_)) => {
                return !crate::shulker::is_item(self.rules.items, stack);
            }
            (MenuKind::BrewingStand, SlotRef::Container(slot)) => {
                return self.rules.brewing_input_allowed(slot, stack);
            }
            (kind, SlotRef::Grid(slot)) if kind.is_craft_station() => {
                return super::craft_stations::allowed(kind, slot, self.rules, stack);
            }
            (MenuKind::EnchantingTable, SlotRef::Grid(1)) => {
                return self.rules.is_lapis(stack);
            }
            (MenuKind::Beacon, SlotRef::Grid(0)) => {
                return self.rules.is_beacon_payment(stack);
            }
            (MenuKind::Grindstone, SlotRef::Grid(_)) => {
                return !stack.enchantments.is_empty()
                    || !stack.extended().stored_enchantments.is_empty()
                    || super::anvil::max_damage(self.rules, stack).is_some();
            }
            _ => {}
        }
        if let Some((kind, slot)) = self.furnace_slot(index) {
            return match slot {
                furnace::FUEL => {
                    furnace::is_fuel(self.rules, kind, stack)
                        || furnace::is_empty_bucket(self.rules, stack)
                }
                furnace::OUTPUT => false,
                _ => true,
            };
        }
        match self.armor_slot(index) {
            Some(slot) => self.rules.equipment(stack) == Some(slot),
            None => true,
        }
    }

    /// Vanilla `Slot.getMaxStackSize(stack)`.
    fn max_size(&self, index: usize, stack: &ItemStack) -> i32 {
        // Vanilla `BeaconMenu.PaymentSlot.getMaxStackSize`.
        if matches!(
            (self.kind, self.slot_ref(index)),
            (MenuKind::EnchantingTable, SlotRef::Grid(0))
                | (MenuKind::BrewingStand, SlotRef::Container(0..=2))
                | (MenuKind::Beacon, SlotRef::Grid(0))
                | (MenuKind::Mount { .. }, SlotRef::Container(0..=1))
        ) {
            return 1;
        }
        if self.armor_slot(index).is_some() {
            return 1;
        }
        if self
            .furnace_slot(index)
            .is_some_and(|(_, slot)| slot == furnace::FUEL)
            && furnace::is_empty_bucket(self.rules, stack)
        {
            return 1;
        }
        self.rules.max_stack(stack).min(CONTAINER_MAX_STACK)
    }

    /// Vanilla `Slot.allowModification`: slots that refuse their own item
    /// (outputs) only give up a stack whole.
    fn allows_partial_take(&self, index: usize) -> bool {
        self.may_place(index, self.get(index))
    }

    /// Vanilla `canItemQuickReplace(slot, stack, true)`.
    fn quick_replace(&self, index: usize, stack: &ItemStack) -> bool {
        let item = self.get(index);
        item.is_empty() || (can_stack(item, stack) && item.count <= self.rules.max_stack(stack))
    }

    fn refresh_result(&mut self) {
        if matches!(self.kind, MenuKind::Player | MenuKind::CraftingTable) {
            *self.result = grid_result(self.rules, self.grid);
        }
        if self.kind.is_craft_station() {
            *self.result =
                super::craft_stations::refresh(self.kind, self.rules, self.grid, self.station);
        }
    }

    fn credit_crafted(&mut self, item_id: u32, count: i32) {
        if count <= 0 {
            return;
        }
        let credit = self.crafted_output.get_or_insert((item_id, 0));
        debug_assert_eq!(credit.0, item_id);
        credit.1 += count;
    }

    /// Furnace outputs use storage slots, not the virtual crafting result.
    fn take_from_slot(&mut self, index: usize, count: i32) -> ItemStack {
        let taken = take_from_stack(self.get_mut(index), count);
        if self
            .furnace_slot(index)
            .is_some_and(|(_, slot)| slot == furnace::OUTPUT)
        {
            self.credit_crafted(taken.item_id, taken.count);
        }
        taken
    }

    /// Vanilla `ResultSlot.onTake`: consume one craft, then recompute.
    fn take_craft(&mut self) {
        // `ItemStack.onCraftedBy` awards ITEM_CRAFTED in these result slots;
        // loom, anvil and grindstone outputs do not invoke that vanilla hook.
        if matches!(
            self.kind,
            MenuKind::Player | MenuKind::CraftingTable | MenuKind::Smithing | MenuKind::Stonecutter
        ) {
            self.credit_crafted(self.result.item_id, self.result.count);
        }
        if matches!(self.kind, MenuKind::Player | MenuKind::CraftingTable)
            && !self.result.is_empty()
        {
            self.crafted
                .push((self.result.item_id, self.result.count, self.kind));
        }
        if self.kind.is_craft_station() {
            let recipe_id = match self.kind {
                MenuKind::Smithing => {
                    self.rules
                        .items
                        .name_of(self.grid[0].item_id)
                        .map(|template| {
                            if template.path() == "netherite_upgrade_smithing_template" {
                                "minecraft:netherite_upgrade_smithing".to_owned()
                            } else {
                                format!("{}_smithing_trim", template.as_str())
                            }
                        })
                }
                MenuKind::Stonecutter => {
                    usize::try_from(self.station.selected)
                        .ok()
                        .and_then(|selected| {
                            self.rules
                                .recipes
                                .iter()
                                .filter(|recipe| {
                                    mc_data::recipes::stonecutting_recipe_accepts_input(
                                        recipe,
                                        self.rules.items,
                                        self.rules.facts,
                                        self.rules.tags,
                                        &self.grid[0],
                                    )
                                })
                                .nth(selected)
                                .map(|recipe| recipe.id.as_str().to_owned())
                        })
                }
                _ => None,
            };
            if let Some(recipe) = recipe_id {
                self.crafted_recipes.push((recipe, self.grid.clone()));
            }
            super::craft_stations::consume(self.kind, self.grid);
            self.refresh_result();
            return;
        }
        if matches!(
            self.kind,
            MenuKind::Anvil | MenuKind::Grindstone | MenuKind::Cartography
        ) {
            *self.station_output_taken = true;
            *self.result = ItemStack::EMPTY;
            return;
        }
        if let Some(input) = super::crafting::grid_input(self.grid)
            && let Some(recipe) = self.rules.recipes.iter().find(|recipe| {
                mc_data::recipes::crafting_recipe_matches(
                    self.rules.items,
                    self.rules.facts,
                    self.rules.tags,
                    &input,
                    recipe,
                )
            })
        {
            self.crafted_recipes
                .push((recipe.id.as_str().to_owned(), self.grid.clone()));
        }
        consume_craft(self.rules, self.grid, self.inventory, self.dropped);
        self.refresh_result();
    }

    // ---- click kinds -----------------------------------------------------

    fn drop_manually(&mut self, stack: ItemStack) {
        if !stack.is_empty() {
            self.manual_drops.push((stack.item_id, stack.count));
            self.dropped.push(stack);
        }
    }

    fn drop_carried(&mut self, whole: bool) {
        if self.carried.is_empty() {
            return;
        }
        let stack = if whole {
            std::mem::replace(self.carried, ItemStack::EMPTY)
        } else {
            take_from_stack(self.carried, 1)
        };
        self.drop_manually(stack);
    }

    fn pickup(&mut self, index: usize, primary: bool) {
        if !self.may_take(index) {
            return;
        }
        if self.is_result(index) {
            self.take_result_to_cursor();
            return;
        }
        let slot_item = self.get(index).clone();
        if slot_item.is_empty() {
            if !self.carried.is_empty() {
                let count = if primary { self.carried.count } else { 1 };
                self.safe_insert(index, count);
            }
        } else if self.carried.is_empty() {
            let count = if primary {
                slot_item.count
            } else {
                (slot_item.count + 1) / 2
            };
            *self.carried = self.take_from_slot(index, count);
        } else if self.may_place(index, &self.carried.clone()) {
            if can_stack(&slot_item, self.carried) {
                let count = if primary { self.carried.count } else { 1 };
                self.safe_insert(index, count);
            } else if self.carried.count <= self.max_size(index, &self.carried.clone()) {
                let carried = std::mem::replace(self.carried, slot_item);
                self.set(index, carried);
            }
        } else if can_stack(&slot_item, self.carried) {
            let room = self.rules.max_stack(self.carried) - self.carried.count;
            if room < slot_item.count && !self.allows_partial_take(index) {
                return;
            }
            let taken = self.take_from_slot(index, room);
            self.carried.count += taken.count;
        }
    }

    /// Vanilla `Slot.safeInsert` from the cursor.
    fn safe_insert(&mut self, index: usize, count: i32) {
        let stack = self.carried.clone();
        if stack.is_empty() || !self.may_place(index, &stack) {
            return;
        }
        let existing = self.get(index).clone();
        let moved = count
            .min(stack.count)
            .min(self.max_size(index, &stack) - existing.count);
        if moved <= 0 {
            return;
        }
        if existing.is_empty() {
            let placed = take_from_stack(self.carried, moved);
            self.set(index, placed);
        } else if can_stack(&existing, &stack) {
            self.carried.count -= moved;
            canonicalize_empty(self.carried);
            self.get_mut(index).count += moved;
        }
    }

    /// The crafting result is taken whole or not at all.
    fn take_result_to_cursor(&mut self) {
        let result = self.result.clone();
        if result.is_empty() {
            return;
        }
        if self.carried.is_empty() {
            *self.carried = result;
        } else if can_stack(&result, self.carried)
            && self.carried.count + result.count <= self.rules.max_stack(self.carried)
        {
            self.carried.count += result.count;
        } else {
            return;
        }
        self.take_craft();
    }

    fn quick_move_click(&mut self, index: usize) {
        let mut moved = self.quick_move(index);
        while !moved.is_empty() && self.get(index).item_id == moved.item_id {
            moved = self.quick_move(index);
        }
    }

    /// Vanilla `quickMoveStack` per menu; returns the original stack when
    /// something moved, else empty.
    fn quick_move(&mut self, index: usize) -> ItemStack {
        if !self.may_take(index) {
            return ItemStack::EMPTY;
        }
        let original = self.get(index).clone();
        if original.is_empty() {
            return ItemStack::EMPTY;
        }
        if self.kind.is_craft_station() && self.is_result(index) {
            let start = self.grid.len() + 1;
            let capacity: i32 = (start..self.kind.slot_count())
                .map(|slot| {
                    let target = self.get(slot);
                    if target.is_empty() {
                        self.max_size(slot, &original)
                    } else if can_stack(target, &original) {
                        (self.max_size(slot, &original) - target.count).max(0)
                    } else {
                        0
                    }
                })
                .sum();
            if capacity < original.count {
                return ItemStack::EMPTY;
            }
        }
        let mut stack = original.clone();
        let moved = match self.kind {
            MenuKind::Player => self.quick_move_player(index, &mut stack),
            MenuKind::Lectern => false,
            MenuKind::Mount { .. } => {
                let end = self.kind.container_size();
                if index < end {
                    self.move_to(&mut stack, end, end + 36, true)
                } else {
                    self.move_to(&mut stack, 1, 2, false)
                        || self.move_to(&mut stack, 0, 1, false)
                        || self.move_to(&mut stack, 2, end, false)
                        || if index < end + 27 {
                            self.move_to(&mut stack, end + 27, end + 36, false)
                        } else {
                            self.move_to(&mut stack, end, end + 27, false)
                        }
                }
            }
            MenuKind::CraftingTable => match index {
                0 => self.move_to(&mut stack, 10, 46, true),
                10..46 => {
                    self.move_to(&mut stack, 1, 10, false)
                        || if index < 37 {
                            self.move_to(&mut stack, 37, 46, false)
                        } else {
                            self.move_to(&mut stack, 10, 37, false)
                        }
                }
                _ => self.move_to(&mut stack, 10, 46, false),
            },
            MenuKind::Crafter => {
                if index < 9 {
                    self.move_to(&mut stack, 9, 45, true)
                } else if index < 45 {
                    self.move_to(&mut stack, 0, 9, false)
                } else {
                    false
                }
            }
            MenuKind::Chest { .. }
            | MenuKind::Hopper
            | MenuKind::Dispenser
            | MenuKind::ShulkerBox => {
                let container = self.kind.container_size();
                if index < container {
                    self.move_to(&mut stack, container, self.kind.slot_count(), true)
                } else {
                    self.move_to(&mut stack, 0, container, false)
                }
            }
            MenuKind::Furnace(kind) => match index {
                furnace::OUTPUT => self.move_to(&mut stack, 3, 39, true),
                furnace::INPUT | furnace::FUEL => self.move_to(&mut stack, 3, 39, false),
                _ if furnace::can_smelt(self.rules, kind, &stack) => {
                    self.move_to(&mut stack, 0, 1, false)
                }
                _ if furnace::is_fuel(self.rules, kind, &stack) => {
                    self.move_to(&mut stack, 1, 2, false)
                }
                3..30 => self.move_to(&mut stack, 30, 39, false),
                _ => self.move_to(&mut stack, 3, 30, false),
            },
            MenuKind::EnchantingTable => match index {
                0..=1 => self.move_to(&mut stack, 2, 38, true),
                _ if self.rules.is_lapis(&stack) => self.move_to(&mut stack, 1, 2, false),
                _ => self.move_to(&mut stack, 0, 1, false),
            },
            MenuKind::Merchant => match index {
                0..=1 => self.move_to(&mut stack, 3, 39, false),
                3..30 => self.move_to(&mut stack, 30, 39, false),
                30..39 => self.move_to(&mut stack, 3, 30, false),
                _ => false,
            },
            // `javap BeaconMenu.quickMoveStack`: payment back to the
            // inventory; a single-count payment moves into slot 0 only when
            // it is empty and accepted; main/hotbar swap sides otherwise.
            MenuKind::Beacon => match index {
                0 => self.move_to(&mut stack, 1, 37, true),
                _ if !self.get(0).is_empty()
                    || !self.rules.is_beacon_payment(&stack)
                    || stack.count != 1 =>
                {
                    if (1..28).contains(&index) {
                        self.move_to(&mut stack, 28, 37, false)
                    } else if (28..37).contains(&index) {
                        self.move_to(&mut stack, 1, 28, false)
                    } else {
                        false
                    }
                }
                _ => self.move_to(&mut stack, 0, 1, false),
            },
            MenuKind::Anvil | MenuKind::Grindstone | MenuKind::Cartography => match index {
                2 => self.move_to(&mut stack, 3, 39, true),
                0..=1 => self.move_to(&mut stack, 3, 39, false),
                _ => self.move_to(&mut stack, 0, 2, false),
            },
            MenuKind::Smithing | MenuKind::Loom | MenuKind::Stonecutter => {
                let inputs = self.grid.len();
                let start = inputs + 1;
                if index == inputs {
                    self.move_to(&mut stack, start, self.kind.slot_count(), true)
                } else if index < inputs {
                    self.move_to(&mut stack, start, self.kind.slot_count(), false)
                } else {
                    self.move_to(&mut stack, 0, inputs, false)
                        || if index < start + 27 {
                            self.move_to(&mut stack, start + 27, self.kind.slot_count(), false)
                        } else {
                            self.move_to(&mut stack, start, start + 27, false)
                        }
                }
            }
            MenuKind::BrewingStand => match index {
                0..=4 => self.move_to(&mut stack, 5, 41, true),
                _ if self.rules.brewing_input_allowed(4, &stack) => {
                    self.move_to(&mut stack, 4, 5, false) || self.move_to(&mut stack, 3, 4, false)
                }
                _ if self.rules.brewing_input_allowed(3, &stack) => {
                    self.move_to(&mut stack, 3, 4, false)
                }
                _ if self.rules.brewing_input_allowed(0, &stack) => {
                    self.move_to(&mut stack, 0, 3, false)
                }
                5..32 => self.move_to(&mut stack, 32, 41, false),
                _ => self.move_to(&mut stack, 5, 32, false),
            },
        };
        if !moved {
            return ItemStack::EMPTY;
        }
        if self.is_result(index) {
            // The craft is consumed once; a part that did not fit is dropped.
            self.take_craft();
            if !stack.is_empty() {
                self.dropped.push(stack);
            }
        } else {
            if self
                .furnace_slot(index)
                .is_some_and(|(_, slot)| slot == furnace::OUTPUT)
            {
                self.credit_crafted(original.item_id, original.count - stack.count);
            }
            self.set(index, stack);
        }
        self.refresh_result();
        original
    }

    fn quick_move_player(&mut self, index: usize, stack: &mut ItemStack) -> bool {
        if index == 0 {
            return self.move_to(stack, 9, 45, true);
        }
        if (1..9).contains(&index) {
            return self.move_to(stack, 9, 45, false);
        }
        let equipment = self.rules.equipment(stack);
        if let Some(slot) = equipment.filter(|slot| *slot != Equipment::OffHand) {
            let target = 44 - slot.inventory_index();
            if self.get(target).is_empty() {
                return self.move_to(stack, target, target + 1, false);
            }
        }
        if equipment == Some(Equipment::OffHand) && self.get(45).is_empty() {
            return self.move_to(stack, 45, 46, false);
        }
        match index {
            9..36 => self.move_to(stack, 36, 45, false),
            36..45 => self.move_to(stack, 9, 36, false),
            _ => self.move_to(stack, 9, 45, false),
        }
    }

    /// Vanilla `moveItemStackTo`: top up matching stacks, then the first
    /// empty slot that accepts the item.
    fn move_to(&mut self, stack: &mut ItemStack, start: usize, end: usize, reverse: bool) -> bool {
        let order: Vec<usize> = if reverse {
            (start..end).rev().collect()
        } else {
            (start..end).collect()
        };
        let mut moved = false;
        if self.rules.max_stack(stack) > 1 {
            for &index in &order {
                if stack.is_empty() {
                    break;
                }
                let dest = self.get(index).clone();
                if dest.is_empty() || !can_stack(&dest, stack) || !self.may_place(index, stack) {
                    continue;
                }
                let limit = self.max_size(index, &dest);
                let total = dest.count + stack.count;
                if total <= limit {
                    self.get_mut(index).count = total;
                    stack.count = 0;
                    moved = true;
                } else if dest.count < limit {
                    stack.count -= limit - dest.count;
                    self.get_mut(index).count = limit;
                    moved = true;
                }
            }
        }
        if !stack.is_empty() {
            for &index in &order {
                if self.get(index).is_empty() && self.may_place(index, stack) {
                    let limit = self.max_size(index, stack);
                    let placed = take_from_stack(stack, stack.count.min(limit));
                    self.set(index, placed);
                    moved = true;
                    break;
                }
            }
        }
        canonicalize_empty(stack);
        moved
    }

    /// Number key (`inventory_slot` 0-8) or off-hand key (40) on a slot.
    fn swap(&mut self, index: usize, inventory_slot: usize) {
        if !self.may_take(index) {
            return;
        }
        let held = self.inventory.slots[inventory_slot].clone();
        let slot_stack = self.get(index).clone();
        if self.kind.is_craft_station() && self.is_result(index) {
            if slot_stack.is_empty() {
                return;
            }
            if held.is_empty() {
                self.inventory.slots[inventory_slot] = slot_stack;
            } else if can_stack(&held, &slot_stack)
                && held.count + slot_stack.count <= self.rules.max_stack(&held)
            {
                self.inventory.slots[inventory_slot].count += slot_stack.count;
            } else {
                return;
            }
            self.take_craft();
            return;
        }
        if held.is_empty() && slot_stack.is_empty() {
            return;
        }
        if held.is_empty() {
            self.inventory.slots[inventory_slot] = slot_stack;
            if self.is_result(index) {
                self.take_craft();
            } else {
                if self
                    .furnace_slot(index)
                    .is_some_and(|(_, slot)| slot == furnace::OUTPUT)
                {
                    self.credit_crafted(self.get(index).item_id, self.get(index).count);
                }
                self.set(index, ItemStack::EMPTY);
            }
        } else if slot_stack.is_empty() {
            if self.may_place(index, &held) {
                let limit = self.max_size(index, &held);
                let placed = take_from_stack(&mut self.inventory.slots[inventory_slot], limit);
                self.set(index, placed);
            }
        } else if self.may_place(index, &held) {
            let limit = self.max_size(index, &held);
            if held.count > limit {
                let placed = take_from_stack(&mut self.inventory.slots[inventory_slot], limit);
                self.set(index, placed);
                let mut displaced = slot_stack;
                self.inventory.add(self.rules, &mut displaced);
                self.drop_manually(displaced);
            } else {
                self.inventory.slots[inventory_slot] = slot_stack;
                self.set(index, held);
            }
        }
    }

    fn throw(&mut self, index: usize, whole: bool) {
        if !self.may_take(index) {
            return;
        }
        if self.is_result(index) {
            // Each throw takes one whole craft; Ctrl+Q repeats while the
            // result stays the same item.
            loop {
                let taken = self.result.clone();
                if taken.is_empty() {
                    return;
                }
                self.take_craft();
                let same = self.result.item_id == taken.item_id;
                self.drop_manually(taken);
                if !whole || !same || self.kind.is_craft_station() {
                    return;
                }
            }
        }
        let count = if whole { self.get(index).count } else { 1 };
        let taken = self.take_from_slot(index, count);
        self.drop_manually(taken);
    }

    /// Double click: gather matching stacks onto the cursor, partial stacks
    /// first, in slot order (`forward`) or reverse.
    fn pickup_all(&mut self, index: usize, forward: bool) {
        if self.carried.is_empty() || !self.get(index).is_empty() {
            return;
        }
        let count = self.kind.slot_count();
        let order: Vec<usize> = if forward {
            (0..count).collect()
        } else {
            (0..count).rev().collect()
        };
        for pass in 0..2 {
            for &candidate in &order {
                let max = self.rules.max_stack(self.carried);
                if self.carried.count >= max {
                    return;
                }
                let item = self.get(candidate).clone();
                if item.is_empty()
                    || !self.may_take(candidate)
                    || self.is_result(candidate)
                    || !can_stack(&item, self.carried)
                    || !self.quick_replace(candidate, &self.carried.clone())
                {
                    continue;
                }
                if pass == 0 && item.count == self.rules.max_stack(&item) {
                    continue;
                }
                let room = max - self.carried.count;
                if room < item.count && !self.allows_partial_take(candidate) {
                    continue;
                }
                let taken = self.take_from_slot(candidate, room);
                self.carried.count += taken.count;
            }
        }
    }

    // ---- drag (quick craft) ----------------------------------------------

    fn reset_quick_craft(&mut self) {
        *self.quick_craft = QuickCraft::default();
    }

    fn quick_craft_click(&mut self, slot: i16, button: i8) {
        let previous = self.quick_craft.status;
        let status = (button & 3).unsigned_abs();
        self.quick_craft.status = status;
        if (previous != 1 || status != 2) && previous != status {
            self.reset_quick_craft();
            return;
        }
        if self.carried.is_empty() {
            self.reset_quick_craft();
            return;
        }
        match status {
            0 => {
                let kind = ((button >> 2) & 3).unsigned_abs();
                if kind == 0 || kind == 1 || (kind == 2 && self.creative) {
                    self.quick_craft.kind = kind;
                    self.quick_craft.status = 1;
                    self.quick_craft.slots.clear();
                } else {
                    self.reset_quick_craft();
                }
            }
            1 => {
                let Some(index) = self.index(slot) else {
                    return;
                };
                let carried = self.carried.clone();
                if !self.is_result(index)
                    && self.quick_replace(index, &carried)
                    && self.may_place(index, &carried)
                    && (self.quick_craft.kind == 2
                        || usize::try_from(carried.count).unwrap_or(0)
                            > self.quick_craft.slots.len())
                    && !self.quick_craft.slots.contains(&index)
                {
                    self.quick_craft.slots.push(index);
                }
            }
            2 => {
                self.finish_quick_craft();
                self.reset_quick_craft();
            }
            _ => self.reset_quick_craft(),
        }
    }

    fn finish_quick_craft(&mut self) {
        let slots = self.quick_craft.slots.clone();
        let kind = self.quick_craft.kind;
        match slots.len() {
            0 => {}
            1 => {
                if kind <= 1 {
                    self.pickup(slots[0], kind == 0);
                }
            }
            len => {
                let template = self.carried.clone();
                let mut remaining = template.count;
                for index in slots {
                    if self.is_result(index)
                        || !self.quick_replace(index, &template)
                        || !self.may_place(index, &template)
                        || (kind != 2 && usize::try_from(template.count).unwrap_or(0) < len)
                    {
                        continue;
                    }
                    let existing = self.get(index).count.max(0);
                    let limit = self
                        .rules
                        .max_stack(&template)
                        .min(self.max_size(index, &template));
                    let share = match kind {
                        0 => template.count / i32::try_from(len).unwrap_or(i32::MAX),
                        1 => 1,
                        _ => self.rules.max_stack(&template),
                    };
                    let placed = (share + existing).min(limit);
                    remaining -= placed - existing;
                    let mut stack = template.clone();
                    stack.count = placed;
                    self.set(index, stack);
                }
                let mut carried = template;
                carried.count = remaining;
                canonicalize_empty(&mut carried);
                *self.carried = carried;
            }
        }
    }
}
