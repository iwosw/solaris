//! Player-local workstations. Inputs live in the menu, never in block storage.

use mc_data::ItemStack;

use super::enchanting::{EnchantmentCatalog, TableOffers};
use super::{ItemRules, Menu, MenuKind};

#[derive(Debug, Clone, PartialEq)]
pub struct StationState {
    pub bookshelves: i32,
    pub properties: Vec<i32>,
    pub name: Option<String>,
    pub cost: i32,
    pub material_cost: i32,
    pub experience: i32,
    pub result_allowed: bool,
    pub selected: i32,
    pub input_item: Option<u32>,
}

impl Default for StationState {
    fn default() -> Self {
        Self {
            bookshelves: 0,
            properties: Vec::new(),
            name: None,
            cost: 0,
            material_cost: 0,
            experience: 0,
            result_allowed: true,
            selected: -1,
            input_item: None,
        }
    }
}

impl MenuKind {
    #[must_use]
    pub fn is_station(self) -> bool {
        matches!(
            self,
            Self::EnchantingTable
                | Self::Anvil
                | Self::Grindstone
                | Self::Smithing
                | Self::Loom
                | Self::Stonecutter
                | Self::Cartography
        )
    }
}

impl Menu {
    /// Recompute from authoritative inputs and player XP. No per-click data IO.
    pub fn refresh_station(
        &mut self,
        catalog: &EnchantmentCatalog,
        rules: &ItemRules<'_>,
        seed: i32,
        creative: bool,
        level: i32,
    ) {
        match self.kind {
            MenuKind::EnchantingTable => {
                let offers = TableOffers::generate(
                    catalog,
                    rules,
                    &self.grid[0],
                    seed,
                    self.station.bookshelves,
                );
                self.station.properties.clear();
                self.station.properties.extend(offers.costs);
                self.station.properties.push(seed & !15);
                self.station.properties.extend(offers.clues);
                self.station.properties.extend(offers.levels);
            }
            MenuKind::Anvil => {
                let output = super::anvil::result(
                    catalog,
                    rules,
                    &self.grid[0],
                    &self.grid[1],
                    self.station.name.as_deref(),
                    creative,
                );
                self.result = output.item;
                self.station.cost = output.cost;
                self.station.material_cost = output.material_cost;
                self.station.result_allowed = creative || level >= output.cost;
                self.station.properties.clear();
                self.station.properties.push(output.cost);
            }
            MenuKind::Grindstone => {
                let output = super::grindstone::result(
                    catalog,
                    rules,
                    &self.grid[0],
                    &self.grid[1],
                    seed as u32 as u64,
                );
                self.result = output.item;
                self.station.experience = output.experience;
                self.station.result_allowed = true;
            }
            kind if kind.is_craft_station() => {
                self.result =
                    super::craft_stations::refresh(kind, rules, &self.grid, &mut self.station);
                self.station.result_allowed = true;
            }
            _ => {}
        }
    }

    pub fn consume_station_output(&mut self) {
        if self.kind == MenuKind::Cartography {
            for input in &mut self.grid {
                mc_data::inventory_semantics_26_1_2::take_from_stack(input, 1);
            }
            self.result = ItemStack::EMPTY;
            return;
        }
        self.grid[0] = ItemStack::EMPTY;
        if self.kind == MenuKind::Anvil {
            let remaining = self.grid[1].count - self.station.material_cost;
            if remaining > 0 {
                self.grid[1].count = remaining;
            } else {
                self.grid[1] = ItemStack::EMPTY;
            }
            self.station.name = None;
        } else {
            self.grid[1] = ItemStack::EMPTY;
        }
        self.result = ItemStack::EMPTY;
    }
}

impl ItemRules<'_> {
    pub(crate) fn brewing_input_allowed(&self, slot: usize, stack: &ItemStack) -> bool {
        if stack.is_empty() {
            return true;
        }
        let Some(item) = super::anvil::logical_item(self, stack) else {
            return false;
        };
        if self.items.name_of(stack.item_id) != Some(item) {
            return false;
        }
        match slot {
            0..=2 => matches!(
                item.path(),
                "potion" | "splash_potion" | "lingering_potion" | "glass_bottle"
            ),
            3 => mc_data::stations::brewing_ingredient(stack, self.items),
            4 => item.path() == "blaze_powder",
            _ => false,
        }
    }
    /// Plugin resources never pay a vanilla station using their wire carrier.
    #[must_use]
    pub fn is_lapis(&self, stack: &ItemStack) -> bool {
        super::anvil::logical_item(self, stack)
            .is_some_and(|id| id.as_str() == "minecraft:lapis_lazuli")
    }
}
