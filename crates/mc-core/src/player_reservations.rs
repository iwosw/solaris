//! Reservation admission stages only the native inventory/menu transaction.
//! The authority validates both physical sources before applying any result.

use super::{ItemUse, PlayerEffects, PlayerState, contents_change};
use crate::boundary::Hand;
use crate::inventory::{ItemRules, Menu, OFF_HAND, PlayerInventory};
use crate::survival::Experience;
use mc_data::ItemStack;

/// One execution of the canonical menu engine, not a prediction to re-execute.
/// Cursor, menu revision, XP, seed and drops are committed together with slots.
pub(crate) struct InventoryTransitionPlan {
    statistics: mc_data::statistics::PlayerStatistics,
    inventory: PlayerInventory,
    inventory_menu: Menu,
    open: Option<Menu>,
    experience: Experience,
    enchantment_seed: i32,
    enchantment_offer_seed: i32,
    using: Option<ItemUse>,
    container: Vec<ItemStack>,
    effects: PlayerEffects,
}

impl InventoryTransitionPlan {
    pub(crate) fn player_slots(&self) -> &[ItemStack] {
        &self.inventory.slots
    }

    pub(crate) fn container_slots(&self) -> &[ItemStack] {
        &self.container
    }
}

/// A single native durability roll and its prepared replacement. In particular,
/// an Unbreaking roll never advances the authoritative seed on a refused break.
pub(crate) struct NativeToolWearPlan {
    slot: usize,
    damage: Option<i32>,
    broken: bool,
    enchantment_seed: i32,
    shield: bool,
}

impl NativeToolWearPlan {
    pub(crate) fn broken_slot(&self) -> Option<usize> {
        self.broken.then_some(self.slot)
    }
}

impl PlayerState {
    /// Use only for inventory/menu methods: click, station click/button, recipe
    /// placement, rename, close, hand swap, drop, creative slot, equipment swap
    /// and harvest_held. World/entity/effect transitions must not run on this
    /// projection.
    /// Call only when a physical source has active holds; the native no-hold
    /// path needs no staging. The callback runs exactly once.
    pub(crate) fn plan_inventory_transition(
        &self,
        container: &[ItemStack],
        transition: impl FnOnce(&mut Self, &mut [ItemStack]) -> PlayerEffects,
    ) -> InventoryTransitionPlan {
        // Equipment use reads cooldowns only for either held stack. Do not copy
        // unrelated cooldown entries or player/world/persistence state.
        let mut item_cooldowns = std::collections::BTreeMap::new();
        for slot in [self.held_slot(Hand::Main), OFF_HAND] {
            let item = self.inventory.slots[slot].item_id;
            if let Some(ticks) = self.item_cooldowns.get(&item) {
                item_cooldowns.insert(item, *ticks);
            }
        }
        let mut candidate = Self {
            connection: self.connection,
            teleport_id: self.teleport_id,
            awaiting_teleport: self.awaiting_teleport,
            dig_progress: None,
            delayed_dig: None,
            opaque_player_fields: Vec::new(),
            statistics: self.statistics.clone(),
            inventory: self.inventory.clone(),
            advancement_progress: std::sync::Arc::clone(&self.advancement_progress),
            unlocked_recipes: std::sync::Arc::clone(&self.unlocked_recipes),
            advancement_inventory: Vec::new(),
            advancement_effects_changed: false,
            advancement_levitation_origin: None,
            inventory_menu: self.inventory_menu.clone(),
            open: self.open.clone(),
            vitals: self.vitals.clone(),
            active_effects: Default::default(),
            enchantment_seed: self.enchantment_seed,
            enchantment_offer_seed: self.enchantment_offer_seed,
            warden_warning: Default::default(),
            game_mode: self.game_mode,
            fall_distance: 0.0,
            fall_flying: self.fall_flying,
            fall_flying_ticks: self.fall_flying_ticks,
            current_impulse_impact_pos: None,
            sprinting: false,
            known_horizontal_movement_squared: 0.0,
            statistics_grounded: self.statistics_grounded,
            airtime_ticks: 0,
            airtime_sample_tick: None,
            airtime_safe_position: None,
            last_vehicle_move_tick: None,
            chat_spam_ticks: self.chat_spam_ticks,
            using: self.using,
            shield_cooldown: self.shield_cooldown,
            item_cooldowns,
            last_contact_position: None,
            respawn_point: None,
            respawn_dimension: self.respawn_dimension,
            respawn_forced: false,
            respawn_yaw: 0.0,
            respawn_pitch: 0.0,
            last_window: self.last_window,
            attribute_equipment: None,
            sent_attributes: Vec::new(),
            soul_speed_active: false,
            attribute_snapshot_held: self.attribute_snapshot_held.clone(),
        };
        let mut container = container.to_vec();
        let effects = transition(&mut candidate, &mut container);
        InventoryTransitionPlan {
            statistics: candidate.statistics,
            inventory: candidate.inventory,
            inventory_menu: candidate.inventory_menu,
            open: candidate.open,
            experience: candidate.vitals.experience,
            enchantment_seed: candidate.enchantment_seed,
            enchantment_offer_seed: candidate.enchantment_offer_seed,
            using: candidate.using,
            container,
            effects,
        }
    }

    /// Apply only after every affected physical source floor has admitted the
    /// plan. No native click, RNG or recipe operation is re-executed here.
    pub(crate) fn apply_inventory_transition(
        &mut self,
        plan: InventoryTransitionPlan,
        container: &mut [ItemStack],
    ) -> PlayerEffects {
        assert_eq!(container.len(), plan.container.len());
        self.statistics = plan.statistics;
        self.inventory = plan.inventory;
        self.inventory_menu = plan.inventory_menu;
        self.open = plan.open;
        self.vitals.experience = plan.experience;
        self.enchantment_seed = plan.enchantment_seed;
        self.enchantment_offer_seed = plan.enchantment_offer_seed;
        self.using = plan.using;
        for (target, stack) in container.iter_mut().zip(plan.container) {
            *target = stack;
        }
        plan.effects
    }

    /// Existing native resync semantics, with no candidate drops, cursor,
    /// revisions, seed, XP or workstation/world effects escaping refusal.
    pub(crate) fn inventory_transition_refused(&self, container: &[ItemStack]) -> PlayerEffects {
        PlayerEffects {
            changes: vec![contents_change(
                self.open.as_ref().unwrap_or(&self.inventory_menu),
                &self.inventory,
                container,
            )],
            ..PlayerEffects::default()
        }
    }

    /// Exact sparse food/drink debit before tick_item_use changes effects,
    /// health, cooldowns, use state or emits the finishing game event.
    pub(crate) fn finishing_consumable_debit(&self) -> Option<(usize, u32)> {
        let using = self.using?;
        let slot = self.held_slot(using.hand);
        let held = &self.inventory.slots[slot];
        (using.remaining == 1
            && !self.creative()
            && !self.spectator()
            && !held.is_empty()
            && held.item_id == using.item_id)
            .then_some((slot, 1))
    }

    /// Drop preflight is a scalar debit; execute drop_selected only after it
    /// passes, without cloning the inventory or the selected stack.
    pub(crate) fn drop_selected_debit(&self, whole_stack: bool) -> Option<(usize, u32)> {
        let slot = self.held_slot(Hand::Main);
        let held = &self.inventory.slots[slot];
        (!self.spectator() && !held.is_empty())
            .then_some((slot, if whole_stack { held.count as u32 } else { 1 }))
    }

    /// Borrowed paired replacements preserve the canonical hand swap. Moving
    /// stock out of the source's addressable slots still requires excess.
    pub(crate) fn hand_swap_replacements(&self) -> Option<[(usize, &ItemStack); 2]> {
        let main = self.held_slot(Hand::Main);
        (!self.spectator()).then_some([
            (main, &self.inventory.slots[OFF_HAND]),
            (OFF_HAND, &self.inventory.slots[main]),
        ])
    }

    /// A creative throw is not a source debit; only a valid slot replacement
    /// needs admission. Invalid inputs retain the ordinary native no-op.
    pub(crate) fn creative_slot_replacement<'a>(
        &self,
        rules: &ItemRules<'_>,
        slot: i16,
        stack: &'a ItemStack,
    ) -> Option<(usize, &'a ItemStack)> {
        if !self.creative() || !super::valid_creative_stack(rules, stack) {
            return None;
        }
        super::creative_inventory_slot(slot).map(|index| (index, stack))
    }

    pub(crate) fn plan_held_tool_wear(
        &self,
        rules: &ItemRules<'_>,
        amount: i32,
    ) -> NativeToolWearPlan {
        self.plan_held_tool_wear_from_seed(rules, amount, self.enchantment_seed)
    }

    pub(crate) fn plan_tool_wear(
        &self,
        rules: &ItemRules<'_>,
        hand: Hand,
        amount: i32,
    ) -> NativeToolWearPlan {
        self.plan_native_wear(
            rules,
            self.held_slot(hand),
            amount,
            false,
            self.enchantment_seed,
        )
    }

    /// Preserve preceding native scalar RNG draws (for example Bane of
    /// Arthropods) without paying any authoritative seed or combat mutation.
    pub(crate) fn plan_held_tool_wear_from_seed(
        &self,
        rules: &ItemRules<'_>,
        amount: i32,
        seed: i32,
    ) -> NativeToolWearPlan {
        self.plan_native_wear(rules, self.held_slot(Hand::Main), amount, false, seed)
    }

    pub(crate) fn plan_shield_wear(
        &self,
        rules: &ItemRules<'_>,
        hand: Hand,
        damage: f32,
    ) -> NativeToolWearPlan {
        let amount = if self.creative() || damage < 3.0 {
            0
        } else {
            (damage.floor() as i32).saturating_add(1)
        };
        self.plan_native_wear(
            rules,
            self.held_slot(hand),
            amount,
            true,
            self.enchantment_seed,
        )
    }

    fn plan_native_wear(
        &self,
        rules: &ItemRules<'_>,
        slot: usize,
        amount: i32,
        shield: bool,
        seed: i32,
    ) -> NativeToolWearPlan {
        let mut plan = NativeToolWearPlan {
            slot,
            damage: None,
            broken: false,
            enchantment_seed: seed,
            shield,
        };
        if amount <= 0 || (!shield && (self.creative() || self.spectator())) {
            return plan;
        }
        let stack = &self.inventory.slots[slot];
        let facts = (!stack.is_empty())
            .then(|| rules.facts.facts_for_stack(stack, rules.items))
            .flatten();
        let (max, armor) = if shield {
            (
                facts.and_then(|facts| facts.max_damage).unwrap_or(336),
                false,
            )
        } else {
            let Some((max, armor)) =
                facts.and_then(|facts| facts.max_damage.map(|max| (max, facts.armor.is_some())))
            else {
                return plan;
            };
            (max, armor)
        };
        let wear = crate::enchantments::durability_damage(
            stack,
            amount,
            armor,
            &mut plan.enchantment_seed,
        );
        if wear == 0 {
            return plan;
        }
        let damage = stack.damage.unwrap_or(0).saturating_add(wear);
        plan.damage = Some(damage);
        plan.broken = if shield {
            damage as u32 >= max
        } else {
            u32::try_from(damage)
                .ok()
                .is_some_and(|damage| damage >= max)
        };
        plan
    }

    pub(crate) fn apply_tool_wear(&mut self, plan: NativeToolWearPlan) -> PlayerEffects {
        // A zero-damage Unbreaking result can still consume RNG.
        self.enchantment_seed = plan.enchantment_seed;
        let Some(damage) = plan.damage else {
            return PlayerEffects::default();
        };
        let before = (!plan.shield).then(|| self.snapshot());
        if plan.broken {
            self.statistics.increment(
                mc_data::statistics::StatType::Broken,
                self.inventory.slots[plan.slot].item_id,
                1,
            );
            self.inventory.slots[plan.slot] = ItemStack::EMPTY;
            if plan.shield {
                self.using = None;
            }
        } else {
            self.inventory.slots[plan.slot].damage = Some(damage);
        }
        PlayerEffects {
            changes: if let Some(before) = before {
                self.sync(&before)
            } else {
                vec![self.inventory_contents()]
            },
            ..PlayerEffects::default()
        }
    }
}
