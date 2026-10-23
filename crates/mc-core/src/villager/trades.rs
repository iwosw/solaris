//! Trade tables and atomic payment/result transitions on the tick owner.
use super::{Gossip, VillagerProfession, VillagerState, VillagerTradeOffer};
use crate::boundary::{ClickMode, EntityId};
use crate::inventory::{ItemRules, Menu, PlayerInventory};
use crate::registry::CoreRegistry;
use mc_data::ItemStack;
use mc_data::inventory_semantics_26_1_2::{can_stack, take_from_stack};
use mc_data::villager_trades_26_1_2::{
    profession_level_offers_26_1_2, villager_level_for_xp_26_1_2,
};
use uuid::Uuid;

pub(crate) fn hero_discount(base: i32, hero: Option<u8>) -> i32 {
    hero.map_or(0, |amplifier| {
        ((base as f32 * (0.3 + 0.0625 * f32::from(amplifier))).floor() as i32).max(1)
    })
}

fn has_predicate(cost: &ItemStack) -> bool {
    !can_stack(cost, &ItemStack::new(cost.item_id, cost.count))
}

impl VillagerTradeOffer {
    pub fn valid(&self) -> bool {
        !self.cost_a.is_empty()
            && self.cost_a.count <= 64
            && self
                .cost_b
                .as_ref()
                .is_none_or(|s| !s.is_empty() && s.count <= 64)
            && !self.result.is_empty()
            && self.result.count <= 64
            && self.uses >= 0
            && self.max_uses > 0
            && self.uses <= self.max_uses
            && self.xp >= 0
            && self.price_multiplier.is_finite()
            && self.price_multiplier >= 0.0
    }

    /// Matches the protocol owner's currently supported ItemCost codec.
    /// Imported exact predicates are retained in NBT, never advertised as an
    /// empty predicate (which would accept the wrong item variant).
    pub(crate) fn wire_supported(&self) -> bool {
        self.valid()
            && !has_predicate(&self.cost_a)
            && self.cost_b.as_ref().is_none_or(|cost| !has_predicate(cost))
    }

    pub fn price(&self, rules: &ItemRules<'_>, reputation: i32) -> i32 {
        self.customer_price(rules, reputation, None)
    }

    pub(crate) fn customer_price(
        &self,
        rules: &ItemRules<'_>,
        reputation: i32,
        hero: Option<u8>,
    ) -> i32 {
        let demand = (self.cost_a.count as f32 * self.demand as f32 * self.price_multiplier)
            .floor()
            .max(0.0) as i32;
        let discount = (reputation as f32 * self.price_multiplier).floor() as i32;
        self.cost_a
            .count
            .saturating_add(demand)
            .saturating_add(self.special_price)
            .saturating_sub(discount)
            .saturating_sub(hero_discount(self.cost_a.count, hero))
            .clamp(1, rules.max_stack(&self.cost_a).clamp(1, 64))
    }

    fn payment_order(
        &self,
        rules: &ItemRules<'_>,
        inputs: &[ItemStack],
        reputation: i32,
        hero: Option<u8>,
    ) -> Option<bool> {
        if !self.valid() || self.uses >= self.max_uses || inputs.len() != 2 {
            return None;
        }
        let matches = |stack: &ItemStack, cost: &ItemStack, count: i32| {
            stack.item_id == cost.item_id && stack.count >= count
                // Empty ItemCost predicate matches all variants of its item.
                && (!has_predicate(cost) || can_stack(stack, cost))
        };
        let order = |a: usize, b: usize| {
            matches(
                &inputs[a],
                &self.cost_a,
                self.customer_price(rules, reputation, hero),
            ) && self.cost_b.as_ref().map_or(inputs[b].is_empty(), |cost| {
                matches(&inputs[b], cost, cost.count)
            })
        };
        if order(0, 1) {
            Some(false)
        } else if order(1, 0) {
            Some(true)
        } else {
            None
        }
    }
}

/// Wire view in stored order. Its index travels only in the view: retained
/// offers (including exact predicates the codec cannot publish) stay untouched.
pub(crate) fn displayed_offers(
    state: &VillagerState,
) -> impl Iterator<Item = (usize, &VillagerTradeOffer)> {
    state
        .offers
        .iter()
        .enumerate()
        .filter(|(_, offer)| offer.wire_supported())
}

pub fn can_trade(state: &VillagerState) -> bool {
    state.age >= 0
        && state.conversion_time.is_none()
        && !state.sleeping
        && (state.wandering
            || !matches!(
                state.data.profession,
                VillagerProfession::None | VillagerProfession::Nitwit
            ))
}

pub fn reputation(state: &VillagerState, customer: Uuid) -> i32 {
    state
        .gossips
        .iter()
        .filter(|g| g.target == customer)
        .fold(0_i32, |value, gossip| {
            let weight: i32 = match gossip.kind.as_str() {
                "major_negative" => -5,
                "minor_negative" => -1,
                "minor_positive" => 1,
                "major_positive" => 5,
                "trading" => 1,
                _ => 0,
            };
            value.saturating_add(gossip.value.saturating_mul(weight))
        })
}

fn unlock(state: &mut VillagerState, registry: &CoreRegistry, level: u8) {
    let profession = match state.data.profession {
        VillagerProfession::Miner => VillagerProfession::Toolsmith,
        VillagerProfession::Lumberjack => VillagerProfession::Fletcher,
        VillagerProfession::Builder => VillagerProfession::Mason,
        VillagerProfession::Soldier => VillagerProfession::Armorer,
        profession => profession,
    };
    let Some(pool) = profession_level_offers_26_1_2(profession.name(), level) else {
        return;
    };
    let mut added = 0;
    for spec in pool {
        if added == 2 || state.offers.len() >= 10 {
            break;
        }
        let (Some(a), Some(result)) = (
            registry.items.id_of(&spec.cost_a.item),
            registry.items.id_of(&spec.result_item),
        ) else {
            continue;
        };
        let cost_b = match spec.cost_b {
            Some(cost) => {
                let Some(id) = registry.items.id_of(&cost.item) else {
                    continue;
                };
                Some(ItemStack::new(id, cost.count))
            }
            None => None,
        };
        let candidate = VillagerTradeOffer {
            cost_a: ItemStack::new(a, spec.cost_a.count),
            cost_b,
            result: ItemStack::new(result, spec.result_count),
            uses: 0,
            max_uses: spec.max_uses,
            reward_exp: true,
            xp: spec.xp,
            special_price: 0,
            demand: 0,
            price_multiplier: spec.price_multiplier,
        };
        if state.offers.iter().any(|o| {
            o.cost_a == candidate.cost_a
                && o.cost_b == candidate.cost_b
                && o.result == candidate.result
        }) {
            continue;
        }
        state.offers.push(candidate);
        added += 1;
    }
}

/// Only remembers an empty initialization attempt, never offer use counts.
/// Holding the weak allocation identity also invalidates a replaced item table.
#[derive(Debug, Clone)]
pub(crate) struct WorkState {
    profession: VillagerProfession,
    level: u8,
    wandering: bool,
    despawn_at: Option<u64>,
    day: i64,
    items: std::sync::Weak<mc_data::items::ItemRegistry>,
}

impl PartialEq for WorkState {
    fn eq(&self, other: &Self) -> bool {
        self.profession == other.profession
            && self.level == other.level
            && self.wandering == other.wandering
            && self.despawn_at == other.despawn_at
            && self.day == other.day
            && self.items.ptr_eq(&other.items)
    }
}

impl WorkState {
    fn matches(&self, state: &VillagerState, registry: &CoreRegistry, day: i64) -> bool {
        self.profession == state.data.profession
            && self.level == state.data.level
            && self.wandering == state.wandering
            && self.despawn_at == state.despawn_at
            && self.day == day
            && self.items.as_ptr() == std::sync::Arc::as_ptr(&registry.items)
    }

    fn empty(state: &VillagerState, registry: &CoreRegistry, day: i64) -> Self {
        Self {
            profession: state.data.profession,
            level: state.data.level,
            wandering: state.wandering,
            despawn_at: state.despawn_at,
            day,
            items: std::sync::Arc::downgrade(&registry.items),
        }
    }
}

pub fn generate(state: &mut VillagerState, registry: &CoreRegistry) {
    if !can_trade(state) {
        return;
    }
    initialize_offers(state, registry);
}

#[cfg(test)]
std::thread_local! {
    static INITIALIZATION_ATTEMPTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Template residents retain their authored merchant even before adulthood.
/// Opening a menu still requires `can_trade`; initialization is not permission.
pub(crate) fn initialize_offers(state: &mut VillagerState, registry: &CoreRegistry) {
    #[cfg(test)]
    INITIALIZATION_ATTEMPTS.set(INITIALIZATION_ATTEMPTS.get() + 1);
    // Menu/template initialization is authoritative even after failed work.
    state.trade_work = None;
    if state.wandering {
        if state.offers.is_empty() {
            let mut rng = crate::natural_spawn::Rng(state.despawn_at.unwrap_or(0));
            state.offers = crate::mob_lanes::trades::generate(registry, &mut rng);
        }
        return;
    }
    if matches!(
        state.data.profession,
        VillagerProfession::None | VillagerProfession::Nitwit
    ) || !state.offers.is_empty()
    {
        return;
    }
    for level in 1..=state.data.level.clamp(1, 5) {
        unlock(state, registry, level);
    }
}

/// Called only while working at the claimed job site. Two restocks per day;
/// vanilla allows the second only strictly after 2,400 game ticks.
pub(crate) fn work(state: &mut VillagerState, registry: &CoreRegistry, tick: u64, day_time: i64) {
    if !can_trade(state) {
        return;
    }
    let day = day_time.div_euclid(24_000);
    if state.offers.is_empty() {
        if !state
            .trade_work
            .as_ref()
            .is_some_and(|cache| cache.matches(state, registry, day))
        {
            initialize_offers(state, registry);
            if state.offers.is_empty() {
                state.trade_work = Some(WorkState::empty(state, registry, day));
            }
        }
    } else {
        state.trade_work = None;
    }
    if state.job_site.is_none() {
        return;
    }
    if day != state.last_restock_day {
        state.restocks_today = 0;
        state.last_restock_day = day;
    }
    let now = i64::try_from(tick).unwrap_or(i64::MAX);
    state.last_work = now;
    if state.restocks_today >= 2
        || (state.restocks_today > 0 && now.saturating_sub(state.last_restock) <= 2_400)
        || !state.offers.iter().any(|o| o.uses > 0)
    {
        return;
    }
    for offer in &mut state.offers {
        offer.demand = offer
            .demand
            .saturating_add(offer.uses)
            .saturating_sub(offer.max_uses - offer.uses);
        offer.uses = 0;
    }
    state.restocks_today += 1;
    state.last_restock = now;
}

pub fn refresh(menu: &mut Menu, state: &VillagerState, rules: &ItemRules<'_>, customer: Uuid) {
    menu.result = menu
        .merchant
        .and_then(|m| state.offers.get(m.selected))
        .filter(|offer| {
            offer
                .payment_order(
                    rules,
                    &menu.grid,
                    reputation(state, customer),
                    menu.merchant.and_then(|m| m.hero),
                )
                .is_some()
        })
        .map_or(ItemStack::EMPTY, |offer| offer.result.clone());
}

/// SelectTrade moves existing payment back first, and fills the selected
/// costs from the player inventory, including offhand. Never creates payment.
pub fn select(
    menu: &mut Menu,
    inventory: &mut PlayerInventory,
    state: &VillagerState,
    rules: &ItemRules<'_>,
    customer: Uuid,
    index: usize,
) {
    let Some(offer) = state.offers.get(index) else {
        return;
    };
    let Some(merchant) = menu.merchant.as_mut() else {
        return;
    };
    merchant.selected = index;
    for input in &mut menu.grid {
        inventory.add(rules, input);
    }
    if menu.grid.iter().any(|s| !s.is_empty()) {
        refresh(menu, state, rules, customer);
        return;
    }
    let mut cost_a = offer.cost_a.clone();
    cost_a.count = offer.customer_price(
        rules,
        reputation(state, customer),
        menu.merchant.and_then(|m| m.hero),
    );
    for (input, cost) in menu
        .grid
        .iter_mut()
        .zip([Some(&cost_a), offer.cost_b.as_ref()])
    {
        let Some(cost) = cost else {
            continue;
        };
        let max = rules.max_stack(cost);
        for slot in (9..36).chain(0..9).chain([crate::inventory::OFF_HAND]) {
            let stack = &mut inventory.slots[slot];
            if stack.item_id != cost.item_id
                || (has_predicate(cost) && !can_stack(stack, cost))
                || (!input.is_empty() && !can_stack(input, stack))
            {
                continue;
            }
            let count = stack.count.min(max - input.count);
            if count <= 0 {
                continue;
            }
            if input.is_empty() {
                *input = stack.clone();
                input.count = 0;
            }
            input.count += count;
            take_from_stack(stack, count);
        }
    }
    refresh(menu, state, rules, customer);
}

#[derive(Debug, Default)]
pub struct TradeOutcome {
    pub trades: i32,
    pub experience: i32,
    pub dropped: Vec<ItemStack>,
}

/// Every successful iteration preflights the entire result destination before
/// either currency is touched. Shift-click therefore cannot partially pay or
/// produce a result when the inventory has insufficient capacity.
pub fn execute(
    menu: &mut Menu,
    inventory: &mut PlayerInventory,
    state: &mut VillagerState,
    registry: &CoreRegistry,
    customer: Uuid,
    button: i8,
    mode: ClickMode,
) -> TradeOutcome {
    let mut outcome = TradeOutcome::default();
    let Some(selection) = menu.merchant.map(|m| m.selected) else {
        return outcome;
    };
    let rules = registry.item_rules();
    let rep = reputation(state, customer);
    let hero = menu.merchant.and_then(|m| m.hero);
    while let Some(offer) = state.offers.get(selection) {
        let Some(swapped) = offer.payment_order(&rules, &menu.grid, rep, hero) else {
            break;
        };
        let result = &offer.result;
        if result.count > rules.max_stack(result) {
            break;
        }
        let fits_stack = |stack: &ItemStack| {
            stack.is_empty()
                || (can_stack(stack, result)
                    && stack.count.saturating_add(result.count) <= rules.max_stack(result))
        };
        let fits = match mode {
            ClickMode::Pickup if button == 0 || button == 1 => fits_stack(&menu.carried),
            ClickMode::QuickMove if button == 0 || button == 1 => {
                let capacity: i32 = (0..36)
                    .map(|slot| {
                        let stack = &inventory.slots[slot];
                        if stack.is_empty() {
                            rules.max_stack(result)
                        } else if can_stack(stack, result) {
                            (rules.max_stack(result) - stack.count).max(0)
                        } else {
                            0
                        }
                    })
                    .sum();
                capacity >= result.count
            }
            ClickMode::Swap if (0..=8).contains(&button) || button == 40 => {
                fits_stack(&inventory.slots[button as usize])
            }
            ClickMode::Throw if menu.carried.is_empty() && (button == 0 || button == 1) => true,
            _ => false,
        };
        if !fits {
            break;
        }
        let result = result.clone();
        let price = offer.customer_price(&rules, rep, hero);
        let cost_b = offer.cost_b.as_ref().map_or(0, |s| s.count);
        let xp = offer.xp;
        let reward = offer.reward_exp;
        take_from_stack(&mut menu.grid[usize::from(swapped)], price);
        if cost_b > 0 {
            take_from_stack(&mut menu.grid[usize::from(!swapped)], cost_b);
        }
        let merge = |target: &mut ItemStack| {
            if target.is_empty() {
                *target = result.clone();
            } else {
                target.count += result.count;
            }
        };
        match mode {
            ClickMode::Pickup => merge(&mut menu.carried),
            ClickMode::Swap => merge(&mut inventory.slots[button as usize]),
            ClickMode::Throw => outcome.dropped.push(result),
            ClickMode::QuickMove => {
                let mut remaining = result;
                for slot in (0..9).rev().chain((9..36).rev()) {
                    let target = &mut inventory.slots[slot];
                    if target.is_empty() || !can_stack(target, &remaining) {
                        continue;
                    }
                    let count = remaining
                        .count
                        .min((rules.max_stack(&remaining) - target.count).max(0));
                    target.count += count;
                    take_from_stack(&mut remaining, count);
                    if remaining.is_empty() {
                        break;
                    }
                }
                for slot in (0..9).rev().chain((9..36).rev()) {
                    if remaining.is_empty() {
                        break;
                    }
                    if inventory.slots[slot].is_empty() {
                        let count = remaining.count.min(rules.max_stack(&remaining));
                        inventory.slots[slot] = remaining.clone();
                        inventory.slots[slot].count = count;
                        take_from_stack(&mut remaining, count);
                    }
                }
                debug_assert!(remaining.is_empty());
            }
            _ => unreachable!(),
        }
        state.offers[selection].uses += 1;
        state.xp = state.xp.saturating_add(xp);
        outcome.trades += 1;
        if reward {
            outcome.experience += 3 + (state.offers[selection].uses % 4);
        }
        if mode != ClickMode::QuickMove {
            break;
        }
    }
    if outcome.trades > 0 {
        let old_level = state.data.level;
        state.data.level = old_level.max(villager_level_for_xp_26_1_2(state.xp));
        if state.data.level > old_level {
            for level in old_level + 1..=state.data.level {
                unlock(state, registry, level);
            }
            outcome.experience += 5;
        }
        if let Some(gossip) = state
            .gossips
            .iter_mut()
            .find(|g| g.target == customer && g.kind == "trading")
        {
            gossip.value = gossip.value.saturating_add(outcome.trades * 2).min(25);
        } else {
            state.gossips.push(Gossip {
                target: customer,
                kind: "trading".into(),
                value: (outcome.trades * 2).min(25),
            });
        }
    }
    refresh(menu, state, &rules, customer);
    outcome
}

pub fn merchant_entity(menu: &Menu) -> Option<EntityId> {
    menu.merchant.map(|m| m.entity)
}

#[cfg(test)]
#[path = "trades_tests.rs"]
mod tests;
