//! Provider-authored attacks execute on the native tick/merge authority.
//! Absolute phases and milli-stamina are retained in the same WAL image as costs.
//! Ballistic contact revalidation shares the exact current-body flight sphere kernel.
use mc_data::ItemStack;
use mc_data::item_profiles::{AttackMode, AttackProfile, PhysicalItemProfile};
use mc_physics::mth;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::boundary::{EntityId, Hand};
use crate::entities::{EntityKind, EntitySlot};
use crate::intent::{DamageSource, Footprint, Intent, IntentKind};
use crate::physical_defense::PhysicalImpact;
use crate::tick::TickState;

const NATIVE: &str = "__native_physical";
const ACTIONS: &str = "attacks";

// Native domain lookup keys stay on the stack, including vanilla flight probes.
struct NativeKey {
    bytes: [u8; 47],
    len: usize,
}
impl NativeKey {
    fn new(prefix: &[u8], uuid: Uuid) -> Self {
        let mut key = Self {
            bytes: [0; 47],
            len: prefix.len() + 36,
        };
        key.bytes[..prefix.len()].copy_from_slice(prefix);
        uuid.hyphenated()
            .encode_lower(&mut key.bytes[prefix.len()..]);
        key
    }
    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).expect("native UUID key")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttackAdmission {
    NotCustom,
    Rejected,
    Accepted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PhysicalStamina {
    available_milli: u32,
    last_tick: u64,
}

fn milli(value: f32) -> u32 {
    (f64::from(value) * 1000.0)
        .round()
        .clamp(0.0, f64::from(u32::MAX)) as u32
}
impl PhysicalStamina {
    fn available(self, capacity: f32, recovery: f32, tick: u64) -> u32 {
        u64::from(self.available_milli)
            .saturating_add(
                u64::from(milli(recovery)).saturating_mul(tick.saturating_sub(self.last_tick)),
            )
            .min(u64::from(milli(capacity))) as u32
    }
    fn reserve(self, capacity: f32, recovery: f32, tick: u64, cost: f32) -> Option<Self> {
        let available = self.available(capacity, recovery, tick);
        (available >= milli(cost)).then(|| Self {
            available_milli: available - milli(cost),
            last_tick: tick,
        })
    }
}
fn stamina_record(state: &TickState, actor: usize, capacity: f32) -> Option<PhysicalStamina> {
    let slot = EntitySlot(u32::try_from(actor).ok()?);
    let uuid = state.entities.row(slot).uuid;
    state
        .plugins
        .get_domain(NATIVE, NativeKey::new(b"stamina/", uuid).as_str())
        .map_or(
            Some(PhysicalStamina {
                available_milli: milli(capacity),
                last_tick: state.tick,
            }),
            |value| PhysicalStamina::deserialize(value).ok(),
        )
}
pub(crate) fn stamina(state: &TickState, actor: usize, capacity: f32, recovery: f32) -> f32 {
    stamina_record(state, actor, capacity).map_or(0.0, |record| {
        record.available(capacity, recovery, state.tick) as f32 / 1000.0
    })
}
pub(crate) fn spend_stamina(
    state: &mut TickState,
    actor: usize,
    capacity: f32,
    recovery: f32,
    cost: f32,
) -> bool {
    let Some(after) = stamina_record(state, actor, capacity)
        .and_then(|record| record.reserve(capacity, recovery, state.tick, cost))
    else {
        return false;
    };
    let uuid = state.entities.row(EntitySlot(actor as u32)).uuid;
    state.plugins.set_domain(
        NATIVE,
        NativeKey::new(b"stamina/", uuid).as_str(),
        serde_json::to_value(after).expect("native stamina"),
    );
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PhysicalAttackPhase {
    started_tick: u64,
    active_tick: u64,
    inactive_tick: u64,
    ready_tick: u64,
    effect_claimed: bool,
}
impl PhysicalAttackPhase {
    fn new(profile: &AttackProfile, tick: u64) -> Self {
        let active_tick = tick.saturating_add(u64::from(profile.windup_ticks));
        Self {
            started_tick: tick,
            active_tick,
            inactive_tick: active_tick.saturating_add(u64::from(profile.active_ticks)),
            ready_tick: profile.ready_tick(tick),
            effect_claimed: false,
        }
    }
    fn claim_active(&mut self, profile: &AttackProfile, tick: u64) -> bool {
        if self.effect_claimed || !profile.active_at(self.started_tick, tick) {
            return false;
        }
        self.effect_claimed = true;
        true
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderApproval {
    operation_id: String,
    order_revision: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    actor: Uuid,
    target: Option<Uuid>,
    held_slot: usize,
    held: ItemStack,
    attack_index: usize,
    phase: PhysicalAttackPhase,
    order: Option<OrderApproval>,
}
fn pending(state: &TickState, uuid: Uuid) -> Option<Pending> {
    Pending::deserialize(
        state
            .plugins
            .get_domain(NATIVE, ACTIONS)?
            .get(NativeKey::new(b"", uuid).as_str())?,
    )
    .ok()
}
fn save_pending(state: &mut TickState, action: &Pending) {
    let key = NativeKey::new(b"", action.actor);
    let value = serde_json::to_value(action).expect("native attack phase");
    if let Some(actions) = state.plugins.get_domain_mut(NATIVE, ACTIONS) {
        let actions = actions
            .as_object_mut()
            .expect("validated native action image");
        if let Some(previous) = actions.get_mut(key.as_str()) {
            *previous = value;
        } else {
            actions.insert(key.as_str().to_owned(), value);
        }
    } else {
        let mut actions = serde_json::Map::new();
        actions.insert(key.as_str().to_owned(), value);
        state
            .plugins
            .set_domain(NATIVE, ACTIONS, serde_json::Value::Object(actions));
    }
}
fn remove_pending(state: &mut TickState, uuid: Uuid) {
    let key = NativeKey::new(b"", uuid);
    if state
        .plugins
        .get_domain(NATIVE, ACTIONS)
        .and_then(|v| v.get(key.as_str()))
        .is_none()
    {
        return;
    }
    let empty = {
        let actions = state
            .plugins
            .get_domain_mut(NATIVE, ACTIONS)
            .expect("native actions")
            .as_object_mut()
            .expect("validated native action image");
        actions.remove(key.as_str());
        actions.is_empty()
    };
    if empty {
        state.plugins.delete_domain(NATIVE, ACTIONS);
    }
}

pub(crate) fn npc_combat_control(
    state: &TickState,
    actor: EntitySlot,
) -> Option<mc_script::ScriptNpcCombatControl> {
    let key = NativeKey::new(b"", state.entities.row(actor).uuid);
    let value = state
        .plugins
        .get_domain(NATIVE, ACTIONS)?
        .get(key.as_str())?;
    let phase = PhysicalAttackPhase::deserialize(value.get("phase")?).ok()?;
    let operation = value.get("order")?.get("operation_id")?.as_str()?;
    Some(mc_script::ScriptNpcCombatControl::new(
        operation.to_owned(),
        phase.started_tick,
        phase.ready_tick,
        None,
        None,
    ))
}

pub(crate) fn validate_profile(profile: &PhysicalItemProfile) -> Result<(), crate::CoreError> {
    if profile
        .attacks
        .iter()
        .any(|attack| attack.brace.is_some() || attack.charge.is_some())
    {
        return Err(crate::CoreError::Plugin(
            "native brace/charge predicates are not admitted".into(),
        ));
    }
    if profile.attacks.iter().any(|attack| {
        attack.projectile.as_ref().is_some_and(|projectile| {
            !matches!(
                projectile.carrier.as_str(),
                "minecraft:arrow" | "minecraft:spectral_arrow"
            )
        })
    }) {
        return Err(crate::CoreError::Plugin(
            "unsupported native physical projectile carrier".into(),
        ));
    }
    crate::physical_defense::validate_profile(profile)
}
fn held(state: &TickState, actor: EntitySlot) -> Option<(usize, &ItemStack)> {
    let row = state.entities.row(actor);
    if let Some(player) = state.players.get(&state.entities.body(actor).id) {
        let index = usize::from(player.inventory.selected.min(8));
        Some((index, player.inventory.slots.get(index)?))
    } else {
        Some((0, row.appearance.equipment[0].as_ref()?))
    }
}
fn physical<'a>(state: &'a TickState, stack: &ItemStack) -> Option<&'a PhysicalItemProfile> {
    state
        .registry
        .item_facts
        .custom_for_stack(stack, &state.registry.items)?
        .facts
        .physical_profile
        .as_deref()
}
fn actor_allowed(state: &TickState, actor: EntitySlot) -> bool {
    let row = state.entities.row(actor);
    state.entities.body(actor).alive
        && row.health > 0.0
        && match state.entities.body(actor).kind {
            EntityKind::Player { .. } => state
                .players
                .get(&state.entities.body(actor).id)
                .is_some_and(|p| !p.spectator()),
            EntityKind::Mob { .. } => row.projectile.is_none(),
            _ => false,
        }
}
fn allowed_target(
    state: &TickState,
    actor: EntitySlot,
    target: EntitySlot,
    order: Option<&OrderApproval>,
) -> bool {
    let source = state.entities.body(actor);
    let victim = state.entities.row(target);
    actor != target
        && actor_allowed(state, actor)
        && state.entities.body(target).alive
        && victim.health > 0.0
        && matches!(
            state.entities.body(target).kind,
            EntityKind::Player { .. } | EntityKind::Mob { .. }
        )
        && victim.projectile.is_none()
        && state
            .players
            .get(&state.entities.body(target).id)
            .is_none_or(|p| !p.spectator() && !p.creative())
        && crate::scoreboard::permits_player_damage(
            state,
            source.id,
            state.entities.body(target).id,
        )
        && order.is_none_or(|order| {
            crate::plugin_npcs::physical_attack_permitted(
                state,
                actor,
                target,
                &order.operation_id,
                order.order_revision,
            )
        })
}
fn body_center(state: &TickState, slot: EntitySlot) -> [f64; 3] {
    let mut pos = state.entities.pos(slot);
    pos[1] += state.entities.body(slot).height * 0.5;
    pos
}

/// Find a witness inside the actual target footprint, never the vanilla reach halo.
/// Each accepted witness independently satisfies the canonical shape predicate.
fn impact_point(
    state: &TickState,
    actor: EntitySlot,
    target: EntitySlot,
    profile: &AttackProfile,
) -> Option<[f64; 3]> {
    let origin = body_center(state, actor);
    let pos = state.entities.pos(target);
    let row = state.entities.body(target);
    let min = [pos[0] - row.half_width, pos[1], pos[2] - row.half_width];
    let max = [
        pos[0] + row.half_width,
        pos[1] + row.height,
        pos[2] + row.half_width,
    ];
    let yaw = state.entities.rot(actor)[0];
    let yaw_rad = f64::from(yaw).to_radians();
    let direction = [
        -f64::from(mth::sin(yaw_rad)),
        0.0,
        f64::from(mth::cos(yaw_rad)),
    ];
    let center = body_center(state, target);
    let projection = ((center[0] - origin[0]) * direction[0]
        + (center[2] - origin[2]) * direction[2])
        .clamp(profile.min_range_blocks, profile.reach_blocks);
    let nearest = std::array::from_fn(|axis| origin[axis].clamp(min[axis], max[axis]));
    let axial = std::array::from_fn(|axis| {
        (origin[axis] + direction[axis] * projection).clamp(min[axis], max[axis])
    });
    let contains = |point| {
        profile.contains(
            origin,
            yaw,
            point,
            state.entities.vel(actor).map(|v| v / 20.0),
            state.entities.ground(actor),
        )
    };
    [nearest, axial, center]
        .into_iter()
        .chain((0..8).map(|corner| {
            std::array::from_fn(|axis| {
                if corner & (1 << axis) == 0 {
                    min[axis]
                } else {
                    max[axis]
                }
            })
        }))
        .find(|point| {
            contains(*point) && crate::projectiles::flight::clear_segment(state, origin, *point)
        })
}

struct CostPlan {
    debits: Vec<(usize, u32)>,
    wear: Option<(usize, Option<i32>)>,
    player: bool,
    projectile_stack: Option<ItemStack>,
}
fn consumption_matches(state: &TickState, stack: &ItemStack, id: &mc_data::Identifier) -> bool {
    !stack.is_empty()
        && if stack.item_model.is_some() {
            state
                .registry
                .item_facts
                .custom_for_stack(stack, &state.registry.items)
                .is_some_and(|item| &item.id == id)
        } else {
            state.registry.items.name_of(stack.item_id) == Some(id)
        }
}
fn custody_stack(state: &TickState, actor: EntitySlot, index: usize) -> Option<&ItemStack> {
    let row = state.entities.row(actor);
    if let Some(player) = state.players.get(&state.entities.body(actor).id) {
        player.inventory.slots.get(index)
    } else if index < 6 {
        row.appearance.equipment.get(index)?.as_ref()
    } else {
        row.villager.as_ref()?.inventory.get(index - 6)?.as_ref()
    }
}

fn plan_costs(
    state: &TickState,
    actor: EntitySlot,
    profile: &AttackProfile,
    held_slot: usize,
    expected: &ItemStack,
    capture_projectile: bool,
) -> Option<CostPlan> {
    let row = state.entities.row(actor);
    let player = state.players.get(&state.entities.body(actor).id);
    let (current_slot, current) = held(state, actor)?;
    if current_slot != held_slot
        || current != expected
        || expected.is_empty()
        || (profile.two_handed && crate::physical_defense::equipment(state, actor, 1).is_some())
    {
        return None;
    }
    let facts = state
        .registry
        .item_facts
        .custom_for_stack(expected, &state.registry.items)?;
    if expected.count != 1
        || expected.damage.is_some_and(|damage| damage < 0)
        || facts
            .facts
            .max_damage
            .is_some_and(|maximum| expected.damage.unwrap_or(0) as u32 >= maximum)
    {
        return None;
    }
    if player.is_none()
        && (profile.wear != 0 || profile.consumption.is_some())
        && !crate::plugin_inventories::can_synchronize_entity_custody(state, actor)
    {
        return None;
    }
    let slots = player.map_or_else(
        || 6 + row.villager.as_ref().map_or(0, |v| v.inventory.len()),
        |player| player.inventory.slots.len(),
    );
    let mut debits = Vec::new();
    let mut projectile_stack = None;
    if let Some(consumption) = &profile.consumption {
        let available = (0..slots)
            .filter_map(|slot| custody_stack(state, actor, slot))
            .filter(|stack| consumption_matches(state, stack, &consumption.item))
            .fold(0_u32, |total, stack| {
                total.saturating_add(stack.count.max(0) as u32)
            });
        if available < consumption.count {
            return None;
        }
        let mut remaining = consumption.count;
        for slot in 0..slots {
            if remaining == 0 {
                break;
            }
            let Some(stack) = custody_stack(state, actor, slot) else {
                continue;
            };
            if !consumption_matches(state, stack, &consumption.item) {
                continue;
            }
            let taken = remaining.min(stack.count.max(0) as u32);
            if taken == 0 {
                continue;
            }
            if capture_projectile
                && profile.mode == AttackMode::Projectile
                && projectile_stack.is_none()
            {
                let mut material = stack.clone();
                material.count = 1;
                projectile_stack = Some(material);
            }
            debits.push((slot, taken));
            remaining -= taken;
        }
    }
    let held_debit = debits
        .iter()
        .find(|(slot, _)| *slot == held_slot)
        .map_or(0, |(_, count)| *count);
    let wear = if profile.wear != 0 && expected.count as u32 > held_debit {
        let maximum = facts.facts.max_damage?;
        let damage = u32::try_from(expected.damage.unwrap_or(0))
            .ok()?
            .saturating_add(profile.wear);
        Some((
            held_slot,
            if damage >= maximum {
                None
            } else {
                Some(i32::try_from(damage).ok()?)
            },
        ))
    } else {
        None
    };
    let broken_slot = wear
        .filter(|(_, damage)| damage.is_none())
        .map(|(slot, _)| slot);
    if player.is_some()
        && crate::plugin_inventories::below_player_debit_floors(
            state,
            state.entities.body(actor).id,
            &debits,
            broken_slot,
        )
    {
        return None;
    }
    Some(CostPlan {
        debits,
        wear,
        player: player.is_some(),
        projectile_stack,
    })
}

fn debit_stack(stack: &mut ItemStack, count: u32) {
    stack.count -= count as i32;
    if stack.count == 0 {
        *stack = ItemStack::EMPTY;
    }
}
fn wear_stack(stack: &mut ItemStack, damage: Option<i32>) {
    if let Some(damage) = damage {
        stack.damage = Some(damage);
    } else {
        *stack = ItemStack::EMPTY;
    }
}
fn npc_custody_slot(
    state: &mut TickState,
    actor: EntitySlot,
    slot: usize,
) -> &mut Option<ItemStack> {
    let row = state.entities.row_mut(actor);
    if slot < 6 {
        &mut std::sync::Arc::make_mut(&mut row.appearance).equipment[slot]
    } else {
        &mut row
            .villager
            .as_mut()
            .expect("preflighted native cargo")
            .inventory[slot - 6]
    }
}
fn apply_costs(state: &mut TickState, actor: EntitySlot, costs: CostPlan) {
    if costs.debits.is_empty() && costs.wear.is_none() {
        return;
    }
    let id = state.entities.body(actor).id;
    if costs.player {
        let player = state.players.get_mut(&id).expect("admitted native player");
        for (slot, count) in costs.debits {
            debit_stack(&mut player.inventory.slots[slot], count);
        }
        if let Some((slot, damage)) = costs.wear {
            wear_stack(&mut player.inventory.slots[slot], damage);
        }
        let connection = player.connection;
        let contents = player.inventory_contents();
        state.push_player(connection, contents);
    } else {
        for (slot, count) in costs.debits {
            let item = npc_custody_slot(state, actor, slot);
            debit_stack(item.as_mut().expect("preflighted native debit"), count);
            if item.as_ref().is_some_and(ItemStack::is_empty) {
                *item = None;
            }
        }
        if let Some((slot, damage)) = costs.wear {
            let item = npc_custody_slot(state, actor, slot);
            wear_stack(item.as_mut().expect("preflighted native wear"), damage);
            if item.as_ref().is_some_and(ItemStack::is_empty) {
                *item = None;
            }
        }
        state.appearance_changed.push(id);
        crate::plugin_inventories::synchronize_entity_custody(state, actor)
            .expect("preflighted native attack custody revision");
    }
}

fn begin(
    state: &mut TickState,
    actor: EntitySlot,
    target: Option<EntitySlot>,
    order: Option<OrderApproval>,
    mode: Option<AttackMode>,
) -> AttackAdmission {
    let Some((held_slot, stack)) = held(state, actor) else {
        return AttackAdmission::NotCustom;
    };
    let Some(profile) = physical(state, stack) else {
        return AttackAdmission::NotCustom;
    };
    if profile.attacks.is_empty() {
        return AttackAdmission::Rejected;
    }
    if !actor_allowed(state, actor) {
        return AttackAdmission::Rejected;
    }
    let uuid = state.entities.row(actor).uuid;
    if state
        .plugins
        .get_domain(NATIVE, ACTIONS)
        .and_then(|v| v.get(NativeKey::new(b"", uuid).as_str()))
        .and_then(|v| v.get("phase"))
        .and_then(|v| v.get("ready_tick"))
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|ready| state.tick < ready)
    {
        return AttackAdmission::Rejected;
    }
    let selected = profile.attacks.iter().enumerate().find(|(_, attack)| {
        mode.is_none_or(|mode| attack.mode == mode)
            && target.is_none_or(|target| {
                allowed_target(state, actor, target, order.as_ref())
                    && impact_point(state, actor, target, attack).is_some()
            })
            && plan_costs(state, actor, attack, held_slot, stack, false).is_some()
            && stamina_record(state, actor.0 as usize, profile.stamina_capacity)
                .and_then(|record| {
                    record.reserve(
                        profile.stamina_capacity,
                        profile.stamina_recovery_per_tick,
                        state.tick,
                        attack.stamina_cost
                            + crate::physical_defense::armor_stamina_cost(state, actor.0 as usize),
                    )
                })
                .is_some()
    });
    let Some((attack_index, attack)) = selected else {
        return AttackAdmission::Rejected;
    };
    let action = Pending {
        actor: uuid,
        target: target.map(|s| state.entities.row(s).uuid),
        held_slot,
        held: stack.clone(),
        attack_index,
        phase: PhysicalAttackPhase::new(attack, state.tick),
        order,
    };
    save_pending(state, &action);
    AttackAdmission::Accepted
}
pub(crate) fn try_attack(
    state: &mut TickState,
    actor: EntitySlot,
    target: EntitySlot,
) -> AttackAdmission {
    // Unauthenticated mobs enter only via retained order approval below.
    if !matches!(state.entities.body(actor).kind, EntityKind::Player { .. }) {
        return if held(state, actor).is_some_and(|(_, stack)| physical(state, stack).is_some()) {
            AttackAdmission::Rejected
        } else {
            AttackAdmission::NotCustom
        };
    }
    begin(state, actor, Some(target), None, None)
}
pub(crate) fn attack(
    state: &mut TickState,
    actor: EntitySlot,
    target: EntitySlot,
    operation_id: &str,
    order_revision: u64,
) -> Option<bool> {
    match begin(
        state,
        actor,
        Some(target),
        Some(OrderApproval {
            operation_id: operation_id.into(),
            order_revision,
        }),
        None,
    ) {
        AttackAdmission::NotCustom => None,
        AttackAdmission::Rejected => Some(false),
        AttackAdmission::Accepted => Some(true),
    }
}
pub(crate) fn try_use(state: &mut TickState, actor: EntitySlot, hand: Hand) -> AttackAdmission {
    if hand != Hand::Main {
        return AttackAdmission::NotCustom;
    }
    // Native defense owns shield use; an armor/shield profile is not a bow.
    if held(state, actor)
        .and_then(|(_, stack)| physical(state, stack))
        .is_none_or(|profile| {
            !profile
                .attacks
                .iter()
                .any(|attack| attack.mode == AttackMode::Projectile)
        })
    {
        return AttackAdmission::NotCustom;
    }
    if !matches!(state.entities.body(actor).kind, EntityKind::Player { .. }) {
        return AttackAdmission::Rejected;
    }
    begin(state, actor, None, None, Some(AttackMode::Projectile))
}
pub(crate) fn try_release(state: &mut TickState, actor: EntitySlot) -> AttackAdmission {
    let uuid = state.entities.row(actor).uuid;
    let Some(action) = pending(state, uuid) else {
        return AttackAdmission::NotCustom;
    };
    let Some(profile) =
        physical(state, &action.held).and_then(|p| p.attacks.get(action.attack_index))
    else {
        return AttackAdmission::Rejected;
    };
    if profile.mode != AttackMode::Projectile {
        return AttackAdmission::NotCustom;
    }
    // The authored windup, not a vanilla carrier's charge curve, owns launch.
    if state.tick
        < action
            .phase
            .started_tick
            .saturating_add(u64::from(profile.windup_ticks))
    {
        remove_pending(state, uuid);
        return AttackAdmission::Rejected;
    }
    if !action.phase.effect_claimed && !profile.active_at(action.phase.started_tick, state.tick) {
        return AttackAdmission::Rejected;
    }
    AttackAdmission::Accepted
}

/// Queue only due retained actions. Idle stamina never writes a tick image.
pub(crate) fn advance(state: &mut TickState, intents: &mut Vec<Intent>) {
    let Some(actions) = state
        .plugins
        .get_domain(NATIVE, ACTIONS)
        .and_then(serde_json::Value::as_object)
    else {
        return;
    };
    let mut due = Vec::new();
    let mut expired = Vec::new();
    for (uuid, value) in actions {
        let Some(phase) = value.get("phase") else {
            continue;
        };
        let Ok(phase) = PhysicalAttackPhase::deserialize(phase) else {
            continue;
        };
        if state.tick >= phase.ready_tick {
            if let Ok(uuid) = Uuid::parse_str(uuid) {
                expired.push(uuid);
            }
            continue;
        }
        if phase.effect_claimed
            || state.tick < phase.active_tick
            || state.tick >= phase.inactive_tick
        {
            continue;
        }
        let Ok(uuid) = Uuid::parse_str(uuid) else {
            continue;
        };
        let Some(slot) = state.entities.slot_of_uuid(uuid) else {
            continue;
        };
        if !matches!(state.entities.body(slot).kind, EntityKind::Player { .. }) {
            let pos = state.entities.pos(slot);
            if !state.world.is_active(
                (pos[0].floor() as i32).div_euclid(16),
                (pos[2].floor() as i32).div_euclid(16),
            ) {
                continue;
            }
        }
        due.push((state.entities.body(slot).id, phase.started_tick));
    }
    for uuid in expired {
        remove_pending(state, uuid);
    }
    for (actor, started_tick) in due {
        intents.push(Intent {
            actor,
            seq: state.next_seq(actor),
            kind: IntentKind::PhysicalAttack { started_tick },
            footprint: Footprint {
                entity_reads: vec![actor],
                ..Footprint::default()
            },
        });
    }
}

pub(crate) fn commit_attack(
    state: &mut TickState,
    actor_id: EntityId,
    started_tick: u64,
) -> Option<(EntitySlot, PhysicalImpact, DamageSource)> {
    let actor = state.entities.slot_of(actor_id)?;
    let mut action = pending(state, state.entities.row(actor).uuid)?;
    if action.phase.started_tick != started_tick {
        return None;
    }
    // The registry is immutable and shared. Borrow mechanics independently of
    // mutable tick state; melee never copies an authored profile.
    let registry = std::sync::Arc::clone(&state.registry);
    let physical = registry
        .item_facts
        .custom_for_stack(&action.held, &registry.items)?
        .facts
        .physical_profile
        .as_deref()?;
    let capacity = physical.stamina_capacity;
    let recovery = physical.stamina_recovery_per_tick;
    let profile = physical.attacks.get(action.attack_index)?;
    if !action.phase.claim_active(profile, state.tick) {
        return None;
    }
    // Claim even a miss once. Replays cannot retry an effect against a later pose.
    save_pending(state, &action);
    if !actor_allowed(state, actor)
        || held(state, actor)
            .is_none_or(|(slot, stack)| slot != action.held_slot || stack != &action.held)
    {
        return None;
    }
    let origin = body_center(state, actor);
    let yaw = state.entities.rot(actor)[0];
    let target = action
        .target
        .and_then(|uuid| state.entities.slot_of_uuid(uuid));
    let point = if action.target.is_some() {
        let target = target?;
        if !allowed_target(state, actor, target, action.order.as_ref()) {
            return None;
        }
        impact_point(state, actor, target, profile)?
    } else {
        let pitch = f64::from(state.entities.rot(actor)[1]).to_radians();
        let yaw = f64::from(yaw).to_radians();
        [
            origin[0] - f64::from(mth::sin(yaw)) * f64::from(mth::cos(pitch)),
            origin[1] - f64::from(mth::sin(pitch)),
            origin[2] + f64::from(mth::cos(yaw)) * f64::from(mth::cos(pitch)),
        ]
    };
    let mut costs = plan_costs(state, actor, profile, action.held_slot, &action.held, true)?;
    let cost =
        profile.stamina_cost + crate::physical_defense::armor_stamina_cost(state, actor.0 as usize);
    let stamina_after = stamina_record(state, actor.0 as usize, capacity)?
        .reserve(capacity, recovery, state.tick, cost)?;
    let impact = PhysicalImpact {
        damage_hp: profile.base_damage_hp,
        kind: profile.damage_kind,
        source: Some(action.actor),
        origin,
        point,
        impulse: profile.projectile.as_ref().map_or(0.0, |p| p.impulse),
        collision_radius: profile
            .projectile
            .as_ref()
            .map_or(0.0, |p| p.collision_radius_blocks),
    };
    if profile.mode == AttackMode::Projectile {
        let projectile = profile.projectile.as_ref()?;
        let kind = crate::projectiles::ProjectileKind::from_name(projectile.carrier.as_str())?;
        let definition = state
            .registry
            .item_facts
            .custom_for_stack(&action.held, &state.registry.items)?
            .id
            .clone();
        let stack = costs.projectile_stack.take().unwrap_or(ItemStack::EMPTY);
        let mut record = crate::projectiles::ProjectileState::new(kind, Some(action.actor), stack);
        record.pickup = projectile.recoverable && !record.stack.is_empty();
        record.damage = profile.base_damage_hp;
        record.data = crate::projectiles::ProjectileData::Physical {
            definition: definition.clone(),
        };
        let delta = std::array::from_fn::<_, 3, _>(|axis| point[axis] - origin[axis]);
        let length = delta.iter().map(|v| v * v).sum::<f64>().sqrt();
        if !length.is_finite() || length <= 1.0e-9 {
            return None;
        }
        let velocity = delta.map(|v| v / length * projectile.speed_blocks_per_tick);
        let id = crate::projectiles::launch(state, record, origin, velocity)?;
        let uuid = state.entities.row(state.entities.slot_of(id)?).uuid;
        state.plugins.set_domain(
            NATIVE,
            NativeKey::new(b"projectile/", uuid).as_str(),
            serde_json::to_value(ProjectileRecord {
                definition,
                profile: profile.clone(),
                impact,
                target: action.target,
                order: action.order,
            })
            .expect("native authored projectile"),
        );
        apply_costs(state, actor, costs);
        state.plugins.set_domain(
            NATIVE,
            NativeKey::new(b"stamina/", action.actor).as_str(),
            serde_json::to_value(stamina_after).expect("native stamina"),
        );
        return None;
    }
    apply_costs(state, actor, costs);
    state.plugins.set_domain(
        NATIVE,
        NativeKey::new(b"stamina/", action.actor).as_str(),
        serde_json::to_value(stamina_after).expect("native stamina"),
    );
    Some((
        target?,
        impact,
        DamageSource::Melee {
            attacker: actor_id,
            position: state.entities.pos(actor),
            yaw,
            bonus_knockback: 0.0,
            sweep: false,
            fire_ticks: 0,
            bane_ticks: 0,
        },
    ))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectileRecord {
    definition: mc_data::Identifier,
    profile: AttackProfile,
    impact: PhysicalImpact,
    target: Option<Uuid>,
    order: Option<OrderApproval>,
}
#[derive(Clone, Copy)]
pub(crate) struct PhysicalProjectile {
    pub gravity: f64,
    pub collision_radius: f64,
    pub lifetime_ticks: u32,
    pub recoverable: bool,
}
pub(crate) fn projectile_profile(
    state: &TickState,
    slot: EntitySlot,
) -> Option<PhysicalProjectile> {
    if !state.registry.item_facts.has_physical_profiles() {
        return None;
    }
    let uuid = state.entities.row(slot).uuid;
    let key = NativeKey::new(b"projectile/", uuid);
    let value = state.plugins.get_domain(NATIVE, key.as_str())?;
    let profile = value.get("profile")?.get("projectile")?;
    Some(PhysicalProjectile {
        gravity: profile.get("gravity_blocks_per_tick_squared")?.as_f64()?,
        collision_radius: profile.get("collision_radius_blocks")?.as_f64()?,
        lifetime_ticks: u32::try_from(profile.get("lifetime_ticks")?.as_u64()?).ok()?,
        recoverable: profile.get("recoverable")?.as_bool()?,
    })
}
pub(crate) fn projectile_hit(
    state: &TickState,
    projectile: EntitySlot,
    target: EntitySlot,
    point: [f64; 3],
) -> Option<PhysicalImpact> {
    let uuid = state.entities.row(projectile).uuid;
    let record = ProjectileRecord::deserialize(
        state
            .plugins
            .get_domain(NATIVE, NativeKey::new(b"projectile/", uuid).as_str())?,
    )
    .ok()?;
    let actor = state.entities.slot_of_uuid(record.impact.source?)?;
    let deployed = crate::plugin_npcs::deployed_projectile_hit(
        state,
        target,
        state.entities.pos(projectile),
        point,
        record.impact.collision_radius,
    );
    let plane_hit = matches!(deployed, Some(Some(_)));
    if plane_hit {
        if !actor_allowed(state, actor) {
            return None;
        }
        if record.order.is_some() {
            let approved = state.entities.slot_of_uuid(record.target?)?;
            if !allowed_target(state, actor, approved, record.order.as_ref()) {
                return None;
            }
        }
    } else if deployed.is_some() || !allowed_target(state, actor, target, record.order.as_ref()) {
        return None;
    }
    let point = if plane_hit {
        point
    } else {
        // Revalidate the actual ballistic step against the current native body
        // with the flight authority's exact sphere kernel. A rounded world-space
        // entry point can lie an ULP outside the radius; testing that point again
        // as a stationary sphere would reject a real contact.
        let pos = state.entities.pos(target);
        let row = state.entities.body(target);
        let min = [pos[0] - row.half_width, pos[1], pos[2] - row.half_width];
        let max = [
            pos[0] + row.half_width,
            pos[1] + row.height,
            pos[2] + row.half_width,
        ];
        let origin = state.entities.pos(projectile);
        let step = state.entities.vel(projectile).map(|value| value / 20.0);
        let entry = crate::projectiles::flight::swept_sphere_enters(
            origin,
            step,
            min,
            max,
            record.impact.collision_radius,
        )?;
        std::array::from_fn(|axis| origin[axis] + step[axis] * entry)
    };
    let distance = point
        .iter()
        .zip(record.impact.origin)
        .map(|(p, o)| (p - o) * (p - o))
        .sum::<f64>()
        .sqrt();
    if !distance.is_finite()
        || distance < record.profile.min_range_blocks
        || distance > record.profile.reach_blocks
    {
        return None;
    }
    Some(PhysicalImpact {
        point,
        ..record.impact
    })
}
pub(crate) fn drop_projectile(state: &mut TickState, uuid: Uuid) {
    state
        .plugins
        .delete_domain(NATIVE, NativeKey::new(b"projectile/", uuid).as_str());
}

/// Validate native-owned replay data before any projectile can select vanilla
/// mechanics. Offline actor records are legitimate; authored definitions are not
/// permitted to change underneath a retained action or launched carrier.
pub(crate) fn validate_restore(state: &TickState) -> Result<(), crate::CoreError> {
    let failure = |key: &str| {
        crate::CoreError::Plugin(format!("invalid native physical replay record {key}"))
    };
    let valid_order = |order: Option<&OrderApproval>| {
        order.is_none_or(|order| {
            !order.operation_id.is_empty()
                && order.order_revision <= mc_script::MAX_SCRIPT_WORLD_TIME
        })
    };
    for (key, value) in state.plugins.domain_entries(NATIVE) {
        if let Some(uuid) = key.strip_prefix("stamina/") {
            Uuid::parse_str(uuid).map_err(|_| failure(key))?;
            let record = PhysicalStamina::deserialize(value).map_err(|_| failure(key))?;
            if record.last_tick > state.tick {
                return Err(failure(key));
            }
        } else if key == ACTIONS {
            let actions = value.as_object().ok_or_else(|| failure(key))?;
            for (uuid, value) in actions {
                let uuid = Uuid::parse_str(uuid).map_err(|_| failure(key))?;
                let action = Pending::deserialize(value).map_err(|_| failure(key))?;
                let profile = physical(state, &action.held).ok_or_else(|| failure(key))?;
                let attack = profile
                    .attacks
                    .get(action.attack_index)
                    .ok_or_else(|| failure(key))?;
                if action.actor != uuid
                    || action.target == Some(uuid)
                    || action.held.count != 1
                    || action.held_slot >= crate::inventory::PLAYER_SLOTS
                    || action.phase.started_tick > state.tick
                    || action.phase.ready_tick != attack.ready_tick(action.phase.started_tick)
                    || action.phase.active_tick
                        != action
                            .phase
                            .started_tick
                            .saturating_add(u64::from(attack.windup_ticks))
                    || action.phase.inactive_tick
                        != action
                            .phase
                            .active_tick
                            .saturating_add(u64::from(attack.active_ticks))
                    || !valid_order(action.order.as_ref())
                    || (action.order.is_some() && action.target.is_none())
                {
                    return Err(failure(key));
                }
                validate_profile(profile)?;
            }
        } else if let Some(uuid) = key.strip_prefix("projectile/") {
            let uuid = Uuid::parse_str(uuid).map_err(|_| failure(key))?;
            let record = ProjectileRecord::deserialize(value).map_err(|_| failure(key))?;
            let definition = state
                .registry
                .item_facts
                .custom(&record.definition)
                .ok_or_else(|| failure(key))?;
            let physical = definition
                .facts
                .physical_profile
                .as_deref()
                .ok_or_else(|| failure(key))?;
            let projectile = record
                .profile
                .projectile
                .as_ref()
                .ok_or_else(|| failure(key))?;
            if record.profile.mode != AttackMode::Projectile
                || !physical.attacks.contains(&record.profile)
                || record.impact.damage_hp != record.profile.base_damage_hp
                || record.impact.kind != record.profile.damage_kind
                || record.impact.collision_radius != projectile.collision_radius_blocks
                || record.impact.impulse != projectile.impulse
                || record.impact.source.is_none()
                || record
                    .impact
                    .origin
                    .iter()
                    .chain(record.impact.point.iter())
                    .any(|value| !value.is_finite())
                || !valid_order(record.order.as_ref())
                || (record.order.is_some() && record.target.is_none())
                || record
                    .target
                    .is_some_and(|target| Some(target) == record.impact.source)
            {
                return Err(failure(key));
            }
            validate_profile(physical)?;
            if let Some(slot) = state.entities.slot_of_uuid(uuid) {
                let native = state
                    .entities
                    .row(slot)
                    .projectile
                    .as_ref()
                    .ok_or_else(|| failure(key))?;
                if native.kind.name() != projectile.carrier.as_str()
                    || native.owner != record.impact.source
                    || native.pickup
                        != (projectile.recoverable && record.profile.consumption.is_some())
                    || (record.profile.consumption.is_some() && native.stack.count != 1)
                    || (record.profile.consumption.is_none() && !native.stack.is_empty())
                    || !matches!(&native.data, crate::projectiles::ProjectileData::Physical { definition } if definition == &record.definition)
                    || record
                        .profile
                        .consumption
                        .as_ref()
                        .is_some_and(|consumption| {
                            !consumption_matches(state, &native.stack, &consumption.item)
                        })
                {
                    return Err(failure(key));
                }
            }
        }
    }
    for slot in state.entities.slots() {
        let Some(native) = &state.entities.row(slot).projectile else {
            continue;
        };
        if let crate::projectiles::ProjectileData::Physical { definition } = &native.data {
            let uuid = state.entities.row(slot).uuid;
            let key = NativeKey::new(b"projectile/", uuid);
            let record = ProjectileRecord::deserialize(
                state
                    .plugins
                    .get_domain(NATIVE, key.as_str())
                    .ok_or_else(|| failure(key.as_str()))?,
            )
            .map_err(|_| failure(key.as_str()))?;
            if definition != &record.definition {
                return Err(failure(key.as_str()));
            }
        }
    }
    crate::physical_defense::validate_restore(state)
}

#[cfg(test)]
#[path = "physical_attacks_tests.rs"]
mod tests;
