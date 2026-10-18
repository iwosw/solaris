//! Player-owned smithing, loom and stonecutter projections and input debits.
use super::stations::StationState;
use super::{ItemRules, Menu, MenuKind};
use mc_data::ItemStack;

impl MenuKind {
    pub(super) fn is_craft_station(self) -> bool {
        matches!(self, Self::Smithing | Self::Loom | Self::Stonecutter)
    }
}

pub(super) fn allowed(
    kind: MenuKind,
    slot: usize,
    rules: &ItemRules<'_>,
    stack: &ItemStack,
) -> bool {
    if stack.is_empty() {
        return true;
    }
    // A plugin resource's vanilla carrier is not a vanilla station ingredient.
    let Some(name) = super::anvil::logical_item(rules, stack) else {
        return false;
    };
    if rules.items.name_of(stack.item_id) != Some(name) {
        return false;
    }
    match kind {
        MenuKind::Smithing => {
            mc_data::stations::smithing_input_allowed(slot, stack, rules.items, rules.tags)
        }
        MenuKind::Loom => match slot {
            0 => name.path().ends_with("_banner"),
            1 => mc_data::stations::dye_color(stack, rules.items).is_some(),
            2 => !mc_data::stations::loom_patterns(stack, rules.items, rules.tags).is_empty(),
            _ => false,
        },
        MenuKind::Stonecutter => {
            slot == 0
                && rules.recipes.iter().any(|recipe| {
                    mc_data::recipes::stonecutting_recipe_accepts_input(
                        recipe,
                        rules.items,
                        rules.facts,
                        rules.tags,
                        stack,
                    )
                })
        }
        _ => false,
    }
}

pub(super) fn refresh(
    kind: MenuKind,
    rules: &ItemRules<'_>,
    grid: &[ItemStack],
    station: &mut StationState,
) -> ItemStack {
    if kind == MenuKind::Stonecutter {
        let identity = (!grid[0].is_empty()).then_some(grid[0].item_id);
        if station.input_item != identity {
            station.selected = -1;
            station.input_item = identity;
        }
        station.properties.clear();
        station.properties.push(station.selected);
        if !allowed(kind, 0, rules, &grid[0]) {
            return ItemStack::EMPTY;
        }
        let Some(selection) = usize::try_from(station.selected).ok() else {
            return ItemStack::EMPTY;
        };
        return rules
            .recipes
            .iter()
            .filter(|recipe| {
                mc_data::recipes::stonecutting_recipe_accepts_input(
                    recipe,
                    rules.items,
                    rules.facts,
                    rules.tags,
                    &grid[0],
                )
            })
            .nth(selection)
            .and_then(|recipe| {
                let count = i32::try_from(recipe.result.count).ok()?;
                (count > 0)
                    .then(|| {
                        rules
                            .items
                            .id_of(&recipe.result.item)
                            .map(|id| ItemStack::new(id, count))
                    })
                    .flatten()
            })
            .unwrap_or(ItemStack::EMPTY);
    }
    if grid
        .iter()
        .enumerate()
        .any(|(slot, stack)| !allowed(kind, slot, rules, stack))
    {
        return ItemStack::EMPTY;
    }
    let input = std::array::from_fn(|slot| grid.get(slot).cloned().unwrap_or(ItemStack::EMPTY));
    match kind {
        MenuKind::Smithing => mc_data::stations::smithing_result(&input, rules.items, rules.tags),
        MenuKind::Loom => {
            station.properties.clear();
            station.properties.push(station.selected);
            mc_data::stations::loom_result(&input, station.selected, rules.items, rules.tags)
        }
        _ => None,
    }
    .unwrap_or(ItemStack::EMPTY)
}

pub(super) fn consume(kind: MenuKind, grid: &mut [ItemStack]) {
    let count = match kind {
        MenuKind::Smithing => 3,
        MenuKind::Loom => 2,
        MenuKind::Stonecutter => 1,
        _ => 0,
    };
    for stack in &mut grid[..count] {
        mc_data::inventory_semantics_26_1_2::take_from_stack(stack, 1);
    }
}

impl Menu {
    /// Buttons select the advertised filtered recipe/pattern list, not global IDs.
    pub fn select_station(&mut self, rules: &ItemRules<'_>, selection: i32) -> bool {
        if !matches!(self.kind, MenuKind::Loom | MenuKind::Stonecutter) {
            return false;
        }
        self.result = refresh(self.kind, rules, &self.grid, &mut self.station);
        let valid = usize::try_from(selection)
            .ok()
            .is_some_and(|selection| match self.kind {
                MenuKind::Loom => {
                    selection
                        < mc_data::stations::loom_patterns(&self.grid[2], rules.items, rules.tags)
                            .len()
                }
                MenuKind::Stonecutter => {
                    selection
                        < rules
                            .recipes
                            .iter()
                            .filter(|recipe| {
                                mc_data::recipes::stonecutting_recipe_accepts_input(
                                    recipe,
                                    rules.items,
                                    rules.facts,
                                    rules.tags,
                                    &self.grid[0],
                                )
                            })
                            .count()
                }
                _ => false,
            });
        self.station.selected = if valid { selection } else { -1 };
        self.result = refresh(self.kind, rules, &self.grid, &mut self.station);
        self.state_id = (self.state_id + 1) & 0x7fff;
        valid
    }
}
