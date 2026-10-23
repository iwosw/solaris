//! Workstation transitions combine menu results and player XP atomically.

use super::{PlayerEffects, PlayerState, contents_change};
use crate::boundary::{ClickMode, PlayerChange};
use crate::inventory::enchanting::{EnchantmentCatalog, apply_offer, next_seed};
use crate::inventory::{ItemRules, MenuKind};

impl PlayerState {
    pub fn initialize_station(
        &mut self,
        catalog: &EnchantmentCatalog,
        rules: &ItemRules<'_>,
        bookshelves: i32,
    ) -> PlayerEffects {
        let creative = self.creative();
        let Some(menu) = self.open.as_mut().filter(|menu| menu.kind.is_station()) else {
            return PlayerEffects::default();
        };
        menu.station.bookshelves = bookshelves;
        menu.refresh_station(
            catalog,
            rules,
            self.enchantment_offer_seed,
            creative,
            self.vitals.experience.level,
        );
        PlayerEffects {
            changes: self.station_properties(),
            ..PlayerEffects::default()
        }
    }

    fn station_properties(&self) -> Vec<PlayerChange> {
        let Some(menu) = &self.open else {
            return Vec::new();
        };
        menu.station
            .properties
            .iter()
            .enumerate()
            .map(|(property, value)| PlayerChange::MenuProperty {
                window: menu.window,
                property: property as i16,
                value: *value as i16,
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn station_click(
        &mut self,
        catalog: &EnchantmentCatalog,
        rules: &ItemRules<'_>,
        window: u8,
        state_id: i32,
        slot: i16,
        button: i8,
        mode: ClickMode,
        bookshelves: i32,
    ) -> PlayerEffects {
        if window != self.current_window() || self.spectator() {
            return self.click(rules, window, state_id, slot, button, mode, &mut []);
        }
        let creative = self.creative();
        let Some(menu) = self.open.as_mut().filter(|menu| menu.kind.is_station()) else {
            return PlayerEffects::default();
        };
        menu.station.bookshelves = bookshelves;
        menu.refresh_station(
            catalog,
            rules,
            self.enchantment_offer_seed,
            creative,
            self.vitals.experience.level,
        );
        let left_before = menu.grid[0].clone();
        let mut effects = self.click(rules, window, state_id, slot, button, mode, &mut []);
        let menu = self
            .open
            .as_mut()
            .expect("station remains open during click");
        if effects.station_output_taken {
            if menu.kind == MenuKind::Anvil {
                if !creative {
                    self.vitals.experience.add_levels(-menu.station.cost);
                }
                effects.anvil_used = !creative;
            } else {
                effects.experience_orbs = menu.station.experience;
            }
            menu.consume_station_output();
        } else if menu.kind == MenuKind::Anvil && menu.grid[0] != left_before {
            menu.station.name = None;
        }
        menu.refresh_station(
            catalog,
            rules,
            self.enchantment_offer_seed,
            creative,
            self.vitals.experience.level,
        );
        // Publish only the post-debit image, never a transient output/input pair.
        effects.changes.clear();
        effects
            .changes
            .push(contents_change(menu, &self.inventory, &[]));
        effects.changes.extend(self.station_properties());
        if effects.station_output_taken {
            effects.changes.push(self.experience_change());
        }
        effects
    }

    /// Native container buttons select loom patterns and filtered stonecutting
    /// offers; enchanting retains its XP/lapis transaction.
    pub fn station_button(
        &mut self,
        catalog: &EnchantmentCatalog,
        rules: &ItemRules<'_>,
        window: u8,
        button: i32,
        bookshelves: i32,
    ) -> PlayerEffects {
        if self.spectator() || self.current_window() != window {
            return PlayerEffects::default();
        }
        if self
            .open
            .as_ref()
            .is_some_and(|menu| menu.kind == MenuKind::EnchantingTable)
        {
            return self.enchant_offer(catalog, rules, window, button, bookshelves);
        }
        let Some(menu) = self
            .open
            .as_mut()
            .filter(|menu| matches!(menu.kind, MenuKind::Loom | MenuKind::Stonecutter))
        else {
            return PlayerEffects::default();
        };
        menu.select_station(rules, button);
        let mut changes = vec![contents_change(menu, &self.inventory, &[])];
        changes.extend(self.station_properties());
        PlayerEffects {
            changes,
            ..PlayerEffects::default()
        }
    }
    pub fn enchant_offer(
        &mut self,
        catalog: &EnchantmentCatalog,
        rules: &ItemRules<'_>,
        window: u8,
        offer: i32,
        bookshelves: i32,
    ) -> PlayerEffects {
        if self.spectator() || self.current_window() != window || !(0..3).contains(&offer) {
            return PlayerEffects::default();
        }
        let creative = self.creative();
        let Some(menu) = self
            .open
            .as_mut()
            .filter(|menu| menu.kind == MenuKind::EnchantingTable)
        else {
            return PlayerEffects::default();
        };
        menu.station.bookshelves = bookshelves;
        menu.refresh_station(
            catalog,
            rules,
            self.enchantment_offer_seed,
            creative,
            self.vitals.experience.level,
        );
        let index = offer as usize;
        let cost = offer + 1;
        let requirement = menu.station.properties[index];
        if requirement <= 0
            || (!creative
                && (self.vitals.experience.level < requirement
                    || self.vitals.experience.level < cost
                    || menu.grid[1].count < cost
                    || !rules.is_lapis(&menu.grid[1])))
        {
            return PlayerEffects::default();
        }
        let Some(item) = apply_offer(
            catalog,
            rules,
            &menu.grid[0],
            self.enchantment_offer_seed,
            bookshelves,
            index,
        ) else {
            return PlayerEffects::default();
        };
        menu.grid[0] = item;
        let enchanted_item = menu.grid[0].clone();
        // Player.onEnchantmentPerformed debits levels even in creative;
        // infinite materials only exempts the lapis and requirement checks.
        self.vitals.experience.add_levels(-cost);
        if !creative {
            mc_data::inventory_semantics_26_1_2::take_from_stack(&mut menu.grid[1], cost);
        }
        self.enchantment_offer_seed = next_seed(self.enchantment_offer_seed);
        self.statistics
            .increment_custom(mc_data::statistics::CustomStat::EnchantItem, 1);
        menu.state_id = (menu.state_id + 1) & 0x7fff;
        menu.refresh_station(
            catalog,
            rules,
            self.enchantment_offer_seed,
            creative,
            self.vitals.experience.level,
        );
        let mut changes = vec![contents_change(menu, &self.inventory, &[])];
        changes.extend(self.station_properties());
        changes.push(self.experience_change());
        PlayerEffects {
            changes,
            enchanted_item: Some((enchanted_item, cost)),
            ..PlayerEffects::default()
        }
    }

    pub fn rename_item(
        &mut self,
        catalog: &EnchantmentCatalog,
        rules: &ItemRules<'_>,
        name: &str,
    ) -> PlayerEffects {
        if self.spectator() {
            return PlayerEffects::default();
        }
        // SharedConstants.filterText strips formatting/control characters; the
        // native anvil accepts at most 50 UTF-16 code units after filtering.
        let filtered: String = name
            .chars()
            .filter(|c| *c >= ' ' && *c != '\u{7f}' && *c != '\u{a7}')
            .collect();
        if filtered.encode_utf16().count() > 50 {
            return PlayerEffects::default();
        }
        let creative = self.creative();
        let Some(menu) = self
            .open
            .as_mut()
            .filter(|menu| menu.kind == MenuKind::Anvil)
        else {
            return PlayerEffects::default();
        };
        if menu.station.name.as_deref() == Some(filtered.as_str()) {
            return PlayerEffects::default();
        }
        menu.station.name = Some(filtered);
        menu.state_id = (menu.state_id + 1) & 0x7fff;
        menu.refresh_station(
            catalog,
            rules,
            self.enchantment_offer_seed,
            creative,
            self.vitals.experience.level,
        );
        let mut changes = vec![contents_change(menu, &self.inventory, &[])];
        changes.extend(self.station_properties());
        PlayerEffects {
            changes,
            ..PlayerEffects::default()
        }
    }
}

#[cfg(test)]
#[path = "stations_tests.rs"]
mod tests;
