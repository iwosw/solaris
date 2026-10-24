//! Admitted, package-independent armor and shield mechanics on native equipment.
//! Health remains the damage owner's responsibility; costs and custody are sealed
//! in its tick's player/entity and native plugin-domain WAL images.

use mc_data::ItemStack;
use mc_data::item_profiles::{
    ArmorSlot, CoverageRegion, DamageKind, PhysicalItemProfile, ShieldProfile,
};
use mc_physics::mth;
use serde::{Deserialize, Serialize};

use crate::boundary::Hand;
use crate::entities::EntitySlot;
use crate::tick::{DamagePiece, TickState};

const OWNER: &str = "__native_physical";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PhysicalImpact {
    pub damage_hp: f32,
    pub kind: DamageKind,
    pub source: Option<uuid::Uuid>,
    pub origin: [f64; 3],
    pub point: [f64; 3],
    pub impulse: f64,
    pub collision_radius: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DefensePose {
    pub position: [f64; 3],
    pub yaw: f32,
    pub body_height: f64,
    pub half_width: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DeployedShieldPose {
    pub position: [f64; 3],
    pub yaw: f32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DefenseGear<'a> {
    pub slot: usize,
    pub profile: &'a PhysicalItemProfile,
    pub damage: u32,
    pub max_damage: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShieldActivation {
    pub slot: usize,
    pub started_tick: u64,
    pub disabled_until: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DefenseWear {
    pub slot: usize,
    pub expected_damage: u32,
    pub updated_damage: u32,
    pub broken: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DefensePlan {
    pub damage_hp: f32,
    pub stamina: f32,
    pub armor_mass: f32,
    pub shield_blocked: bool,
    pub shield_disabled_until: Option<u64>,
    pub wear: [Option<DefenseWear>; 6],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActiveShield {
    slot: usize,
    started_tick: u64,
    stack: ItemStack,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DefenseState {
    active: Option<ActiveShield>,
    disabled_until: u64,
}

/// Native order is main, off, feet, legs, chest, head, not v1's order.
pub(crate) const fn armor_equipment_index(slot: ArmorSlot) -> usize {
    match slot {
        ArmorSlot::Head => 5,
        ArmorSlot::Chest => 4,
        ArmorSlot::Legs => 3,
        ArmorSlot::Feet => 2,
    }
}

fn player_slot(state: &TickState, actor: EntitySlot, equipment: usize) -> Option<usize> {
    if !matches!(
        state.entities.body(actor).kind,
        crate::entities::EntityKind::Player { .. }
    ) {
        return None;
    }
    let player = state.players.get(&state.entities.body(actor).id)?;
    Some(match equipment {
        0 => usize::from(player.inventory.selected.min(8)),
        1 => crate::inventory::OFF_HAND,
        2..=5 => 34 + equipment,
        _ => return None,
    })
}

pub(crate) fn equipment(state: &TickState, actor: EntitySlot, slot: usize) -> Option<&ItemStack> {
    if let Some(index) = player_slot(state, actor, slot) {
        return state
            .players
            .get(&state.entities.body(actor).id)?
            .inventory
            .slots
            .get(index)
            .filter(|stack| !stack.is_empty());
    }
    state
        .entities
        .row(actor)
        .appearance
        .equipment
        .get(slot)?
        .as_ref()
        .filter(|stack| !stack.is_empty())
}

fn gear(state: &TickState, actor: EntitySlot, slot: usize) -> Option<DefenseGear<'_>> {
    let stack = equipment(state, actor, slot)?;
    if stack.count != 1 {
        return None;
    }
    let resolved = state
        .registry
        .item_facts
        .custom_for_stack(stack, &state.registry.items)?;
    let profile = resolved.facts.physical_profile.as_deref()?;
    if !(profile
        .armor
        .as_ref()
        .is_some_and(|armor| armor_equipment_index(armor.slot) == slot)
        || (slot < 2 && profile.shield.is_some()))
    {
        return None;
    }
    Some(DefenseGear {
        slot,
        profile,
        damage: stack.damage.unwrap_or(0).max(0) as u32,
        max_damage: resolved.facts.max_damage?,
    })
}

pub(crate) fn armor_mass(state: &TickState, actor: usize) -> f32 {
    let actor = EntitySlot(actor as u32);
    (2..6)
        .filter_map(|slot| gear(state, actor, slot))
        .filter(|item| item.damage < item.max_damage)
        .filter_map(|item| item.profile.armor.as_ref())
        .map(|armor| armor.mass_units)
        .sum()
}

/// Moving already-accepted native custody performs work against the same
/// stamina record as attacks and guards. Excess work drains, never creates
/// negative stamina or retroactively changes admitted movement.
pub(crate) fn accepted_movement(state: &mut TickState, actor: EntitySlot, distance: f64) {
    if !distance.is_finite()
        || distance <= 0.0
        || !state.registry.item_facts.has_physical_profiles()
    {
        return;
    }
    let row = state.entities.row(actor);
    let player = matches!(
        state.entities.body(actor).kind,
        crate::entities::EntityKind::Player { .. }
    );
    if !state.entities.body(actor).alive
        || (!player && row.appearance.equipment[2..6].iter().all(Option::is_none))
        || (player
            && state
                .players
                .get(&state.entities.body(actor).id)
                .is_some_and(|player| player.creative() || player.spectator()))
    {
        return;
    }
    let Some(first) = (2..6)
        .filter_map(|slot| gear(state, actor, slot))
        .find(|item| item.damage < item.max_damage)
    else {
        return;
    };
    let mut capacity = first.profile.stamina_capacity;
    let mut recovery = first.profile.stamina_recovery_per_tick;
    for item in (0..6)
        .filter_map(|slot| gear(state, actor, slot))
        .filter(|item| item.damage < item.max_damage)
    {
        if item.profile.stamina_capacity > capacity {
            capacity = item.profile.stamina_capacity;
            recovery = item.profile.stamina_recovery_per_tick;
        }
    }
    let mass = armor_mass(state, actor.0 as usize);
    if mass <= 0.0 {
        return;
    }
    let available = crate::physical_attacks::stamina(state, actor.0 as usize, capacity, recovery);
    let cost = (f64::from(mass) * distance).min(f64::from(available)) as f32;
    if cost > 0.0 {
        let spent = crate::physical_attacks::spend_stamina(
            state,
            actor.0 as usize,
            capacity,
            recovery,
            cost,
        );
        debug_assert!(spent, "single tick owner reserved movement stamina");
    }
}

/// All admitted defense fields execute natively. Range/identity validation is
/// performed by PhysicalItemProfile::validate at canonical item admission.
pub(crate) fn validate_profile(_profile: &PhysicalItemProfile) -> Result<(), crate::CoreError> {
    Ok(())
}

/// Validate retained guard custody before recovery may execute native effects.
/// Live equipment need not match: release, offline custody and death can leave
/// inactive records, and every use/hit separately fences the actual full stack.
pub(crate) fn validate_restore(state: &TickState) -> Result<(), crate::CoreError> {
    for (key, value) in state.plugins.domain_entries(OWNER) {
        let Some(identity) = key.strip_prefix("defense/") else {
            continue;
        };
        let invalid =
            || crate::CoreError::Plugin(format!("invalid native physical defense image {key}"));
        let uuid = uuid::Uuid::parse_str(identity).map_err(|_| invalid())?;
        let mut encoded = [0_u8; 36];
        if uuid.hyphenated().encode_lower(&mut encoded) != identity {
            return Err(invalid());
        }
        let image = DefenseState::deserialize(value).map_err(|_| invalid())?;
        if let Some(active) = image.active {
            if active.slot >= 2
                || active.started_tick > state.tick
                || active.started_tick < image.disabled_until
                || active.stack.count != 1
            {
                return Err(invalid());
            }
            let definition = state
                .registry
                .item_facts
                .custom_for_stack(&active.stack, &state.registry.items)
                .ok_or_else(invalid)?;
            let profile = definition
                .facts
                .physical_profile
                .as_deref()
                .ok_or_else(invalid)?;
            if !profile.shield.as_ref().is_some_and(|shield| shield.manual)
                || definition
                    .facts
                    .max_damage
                    .is_none_or(|max| active.stack.damage.unwrap_or(0).max(0) as u32 >= max)
                || active.stack.damage.is_some_and(|damage| damage < 0)
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

fn key(state: &TickState, actor: EntitySlot) -> String {
    format!("defense/{}", state.entities.row(actor).uuid)
}

pub(crate) fn armor_stamina_cost(state: &TickState, actor: usize) -> f32 {
    let actor = EntitySlot(actor as u32);
    (2..6)
        .filter_map(|slot| gear(state, actor, slot))
        .filter(|item| item.damage < item.max_damage)
        .filter_map(|item| item.profile.armor.as_ref())
        .map(|armor| armor.stamina_cost)
        .sum()
}
fn defense_state(state: &TickState, actor: EntitySlot) -> DefenseState {
    state
        .plugins
        .get_domain(OWNER, &key(state, actor))
        .and_then(|image| DefenseState::deserialize(image).ok())
        .unwrap_or_default()
}
fn save(state: &mut TickState, actor: EntitySlot, image: &DefenseState) {
    state.plugins.set_domain(
        OWNER,
        &key(state, actor),
        serde_json::to_value(image).expect("native defense image serializes"),
    );
}

/// Called after player input or NPC ownership/permit admission. Repeated start
/// retains the absolute activation tick only for the exact same full stack.
pub(crate) fn set_item_use(
    state: &mut TickState,
    actor: EntitySlot,
    hand: Hand,
    active: bool,
) -> bool {
    if !state.registry.item_facts.has_physical_profiles() {
        return false;
    }
    let slot = match hand {
        Hand::Main => 0,
        Hand::Off => 1,
    };
    let mut image = defense_state(state, actor);
    if !active {
        let was_active = image.active.take().is_some();
        if was_active {
            save(state, actor, &image);
        }
        return was_active
            || gear(state, actor, slot).is_some_and(|item| {
                item.profile
                    .shield
                    .as_ref()
                    .is_some_and(|shield| shield.manual)
            });
    }
    let Some(item) = gear(state, actor, slot) else {
        return false;
    };
    if !item
        .profile
        .shield
        .as_ref()
        .is_some_and(|shield| shield.manual)
        || item.damage >= item.max_damage
        || state.tick < image.disabled_until
    {
        return false;
    }
    let stack = equipment(state, actor, slot).expect("admitted equipment");
    if image
        .active
        .as_ref()
        .is_some_and(|current| current.slot == slot && current.stack == *stack)
    {
        return true;
    }
    image.active = Some(ActiveShield {
        slot,
        started_tick: state.tick,
        stack: stack.clone(),
    });
    save(state, actor, &image);
    true
}

pub(crate) fn item_use(
    state: &TickState,
    actor: EntitySlot,
) -> Option<mc_script::ScriptNpcItemUseControl> {
    let image = defense_state(state, actor);
    let active = image.active.as_ref().filter(|active| {
        state.tick >= image.disabled_until
            && equipment(state, actor, active.slot).is_some_and(|stack| *stack == active.stack)
            && gear(state, actor, active.slot).is_some_and(|item| {
                item.damage < item.max_damage
                    && item
                        .profile
                        .shield
                        .as_ref()
                        .is_some_and(|shield| shield.manual)
            })
    });
    if active.is_none() && image.disabled_until == 0 {
        return None;
    }
    Some(mc_script::ScriptNpcItemUseControl::new(
        active.map(|active| active.slot as u8),
        active.map(|active| active.started_tick),
        image.disabled_until,
    ))
}

/// Manual shield admission precedes the native raw hurt window. True means
/// fully blocked; hook deadlines still commit when impulse or stamina bypasses.
pub(crate) fn block_damage(
    state: &mut TickState,
    actor: EntitySlot,
    piece: &mut DamagePiece,
) -> bool {
    !apply_defense(state, actor, piece, true)
}

/// Armor is evaluated only after native land_hit has admitted the raw amount.
pub(crate) fn reduce_damage(
    state: &mut TickState,
    actor: EntitySlot,
    piece: &mut DamagePiece,
) -> bool {
    apply_defense(state, actor, piece, false)
}

fn apply_defense(
    state: &mut TickState,
    actor: EntitySlot,
    piece: &mut DamagePiece,
    shields: bool,
) -> bool {
    // WOLF-ARMOR-ABSORB (vanilla-like): equipped wolf body armor halves
    // incoming damage and wears by 1 per damaging hit, breaking at max
    // durability (armadillo scutes repair it through the interact path).
    if !shields && piece.amount > 0.0 {
        let kind = crate::mounts::type_id(state.entities.body(actor))
            .and_then(|id| state.registry.blueprint(id))
            .map(|blueprint| blueprint.name.as_str());
        if kind == Some("minecraft:wolf")
            && state.entities.row(actor).loot.body_armor.is_some()
        {
            piece.amount *= 0.5;
            let max = state
                .registry
                .item_rules()
                .facts
                .facts_for_stack(
                    state.entities.row(actor).loot.body_armor.as_ref().expect("armor"),
                    &state.registry.items,
                )
                .and_then(|facts| facts.max_damage);
            let row = state.entities.row_mut(actor);
            let armor = row.loot.body_armor.as_mut().expect("armor");
            let worn = armor.damage.unwrap_or(0).saturating_add(1);
            if max.is_some_and(|max| worn as u32 >= max) {
                row.loot.body_armor = None;
            } else {
                armor.damage = Some(worn);
            }
        }
    }
    if !state.registry.item_facts.has_physical_profiles() {
        return true;
    }
    let slots = if shields { 0..2 } else { 2..6 };
    let first = slots.clone().find_map(|slot| gear(state, actor, slot));
    let Some(first) = first else {
        return true;
    };
    let impact = match piece.physical {
        Some(mut impact) => {
            impact.damage_hp = piece.amount;
            impact
        }
        None => {
            let Some(impact) = native_impact(state, actor, piece) else {
                return true;
            };
            impact
        }
    };
    let row = state.entities.row(actor);
    if !state.entities.body(actor).alive
        || row.controls.invulnerable
        || state.tick < row.protected_until
        || state
            .players
            .get(&state.entities.body(actor).id)
            .is_some_and(|player| player.creative() || player.spectator())
    {
        return true;
    }
    let pose = DefensePose {
        position: state.entities.pos(actor),
        yaw: state.entities.rot(actor)[0],
        body_height: state.entities.body(actor).height,
        half_width: state.entities.body(actor).half_width,
    };
    let mut admitted = [first; 6];
    let mut len = 0;
    let mut stamina_profile = first.profile;
    for slot in slots {
        if let Some(item) = gear(state, actor, slot) {
            if item.profile.stamina_capacity > stamina_profile.stamina_capacity {
                stamina_profile = item.profile;
            }
            admitted[len] = item;
            len += 1;
        }
    }
    let capacity = stamina_profile.stamina_capacity;
    let recovery = stamina_profile.stamina_recovery_per_tick;
    let available = crate::physical_attacks::stamina(state, actor.0 as usize, capacity, recovery);
    let mut image = if shields {
        defense_state(state, actor)
    } else {
        DefenseState::default()
    };
    let activation = image
        .active
        .as_ref()
        .filter(|active| {
            equipment(state, actor, active.slot).is_some_and(|stack| *stack == active.stack)
        })
        .map(|active| ShieldActivation {
            slot: active.slot,
            started_tick: active.started_tick,
            disabled_until: image.disabled_until,
        });
    let Some(plan) = plan_physical_defense(
        pose,
        impact,
        &admitted[..len],
        activation,
        available,
        state.tick,
    ) else {
        return true;
    };
    if shields
        && plan.wear.iter().flatten().any(|wear| {
            wear.broken
                && player_slot(state, actor, wear.slot).is_some_and(|slot| {
                    crate::plugin_inventories::below_player_debit_floors(
                        state,
                        state.entities.body(actor).id,
                        &[],
                        Some(slot),
                    )
                })
        })
    {
        // This guard transaction cannot debit custodied stock. The real body
        // hit remains eligible; no absorption, resource charge or wear commits.
        return true;
    }
    if plan.wear.iter().any(Option::is_some)
        && !crate::plugin_inventories::can_synchronize_entity_custody(state, actor)
    {
        crate::plugin::fail_native(state);
        return false;
    }
    // Evaluate ordinary armor/enchantments before custom wear can break a stack.
    // These helpers exclude only custom physical armor stats, not other slots.
    let final_damage = if shields {
        plan.damage_hp
    } else {
        let rules = state.registry.item_rules();
        let breach = crate::mace::breach(piece.source);
        if let Some(player) = state.players.get(&state.entities.body(actor).id) {
            crate::survival::reduce_damage_after_physical(
                &rules,
                &player.inventory.slots[36..40],
                plan.damage_hp,
                piece.kind,
                breach,
            )
        } else {
            crate::entity_loot::reduce_damage_after_physical(
                &rules,
                state.entities.row(actor),
                plan.damage_hp,
                piece.kind,
                breach,
            )
        }
    };
    let cost = (available - plan.stamina).max(0.0);
    if cost > 0.0 {
        let spent = crate::physical_attacks::spend_stamina(
            state,
            actor.0 as usize,
            capacity,
            recovery,
            cost,
        );
        debug_assert!(spent, "single tick owner reserved defense stamina");
        if !spent {
            return true;
        }
    }
    let mut worn = false;
    for wear in plan.wear.into_iter().flatten() {
        worn |= apply_wear(state, actor, wear);
    }
    let mut changed = false;
    if let Some(deadline) = plan.shield_disabled_until {
        changed = image.disabled_until < deadline || image.active.is_some();
        image.disabled_until = image.disabled_until.max(deadline);
        image.active = None;
    } else if let Some(active) = &mut image.active {
        if activation.is_none() {
            image.active = None;
            changed = true;
        } else if let Some(stack) = equipment(state, actor, active.slot) {
            if active.stack != *stack {
                active.stack = stack.clone();
                changed = true;
            }
        } else {
            image.active = None;
            changed = true;
        }
    }
    if changed {
        save(state, actor, &image);
    }
    if worn {
        let id = state.entities.body(actor).id;
        crate::plugin_inventories::synchronize_entity_custody(state, actor)
            .expect("validated native equipment custody ledger advances with physical wear");
        state.appearance_changed.push(id);
        if let Some(player) = state.players.get(&id) {
            let connection = player.connection;
            let change = player.inventory_contents();
            state.outbox.entry(connection).or_default().push(change);
        }
    }
    piece.amount = final_damage;
    if !shields {
        piece.custom_armor = true;
    }
    if !shields {
        piece.physical = Some(impact);
    }
    piece.amount > 0.0
}

fn apply_wear(state: &mut TickState, actor: EntitySlot, wear: DefenseWear) -> bool {
    let Some(stack) = equipment(state, actor, wear.slot) else {
        return false;
    };
    if stack.damage.unwrap_or(0).max(0) as u32 != wear.expected_damage {
        return false;
    }
    if let Some(index) = player_slot(state, actor, wear.slot) {
        let stack = &mut state
            .players
            .get_mut(&state.entities.body(actor).id)
            .expect("player equipment")
            .inventory
            .slots[index];
        if wear.broken {
            *stack = ItemStack::EMPTY;
        } else {
            stack.damage = Some(wear.updated_damage as i32);
        }
    } else {
        let appearance = std::sync::Arc::make_mut(&mut state.entities.row_mut(actor).appearance);
        if wear.broken {
            appearance.equipment[wear.slot] = None;
        } else {
            appearance.equipment[wear.slot]
                .as_mut()
                .expect("native equipment")
                .damage = Some(wear.updated_damage as i32);
        }
    }
    true
}

pub(crate) fn impact_from_piece(
    state: &TickState,
    actor: EntitySlot,
    piece: &DamagePiece,
) -> Option<PhysicalImpact> {
    if let Some(mut impact) = piece.physical {
        impact.damage_hp = piece.amount;
        Some(impact)
    } else {
        native_impact(state, actor, piece)
    }
}

fn native_impact(
    state: &TickState,
    actor: EntitySlot,
    piece: &DamagePiece,
) -> Option<PhysicalImpact> {
    use crate::intent::DamageSource;
    use crate::survival::DamageKind as NativeKind;
    let kind = match piece.kind {
        NativeKind::Melee | NativeKind::Thorns => DamageKind::Crush,
        NativeKind::Projectile | NativeKind::Cactus | NativeKind::SweetBerryBush => {
            DamageKind::Pierce
        }
        NativeKind::Fireball
        | NativeKind::Lightning
        | NativeKind::InFire
        | NativeKind::OnFire
        | NativeKind::Lava
        | NativeKind::HotFloor
        | NativeKind::Campfire => DamageKind::Fire,
        NativeKind::Explosion => DamageKind::Blast,
        _ => return None,
    };
    let row = state.entities.body(actor);
    let mut point = state.entities.pos(actor);
    point[1] += row.height * 0.6;
    let (origin, source, impulse) = match piece.source {
        Some(DamageSource::Melee {
            attacker,
            position,
            bonus_knockback,
            ..
        })
        | Some(DamageSource::Mace {
            attacker,
            position,
            bonus_knockback,
            ..
        }) => {
            let source = state
                .entities
                .slot_of(attacker)
                .map(|slot| state.entities.row(slot).uuid);
            (
                position,
                source,
                crate::combat::HIT_KNOCKBACK + bonus_knockback.max(0.0),
            )
        }
        Some(DamageSource::Sweep {
            attacker, position, ..
        }) => (
            position,
            state
                .entities
                .slot_of(attacker)
                .map(|slot| state.entities.row(slot).uuid),
            crate::combat::HIT_KNOCKBACK,
        ),
        Some(DamageSource::Projectile {
            projectile,
            owner,
            velocity,
            punch,
            ..
        }) => {
            let origin = state
                .entities
                .slot_of_any(projectile)
                .map(|slot| state.entities.pos(slot))
                .unwrap_or([point[0] - velocity[0], point[1], point[2] - velocity[1]]);
            (
                origin,
                state
                    .entities
                    .slot_of(owner)
                    .map(|slot| state.entities.row(slot).uuid),
                crate::combat::HIT_KNOCKBACK + f64::from(punch) * 0.6,
            )
        }
        Some(DamageSource::Explosion { kill_credit, .. }) => (
            point,
            kill_credit.and_then(|(source, _)| {
                state
                    .entities
                    .slot_of_any(source)
                    .map(|slot| state.entities.row(slot).uuid)
            }),
            0.0,
        ),
        Some(DamageSource::Command {
            position, cause, ..
        }) => (
            position.unwrap_or(point),
            cause
                .and_then(|(entity, _)| state.entities.slot_of_any(entity))
                .map(|slot| state.entities.row(slot).uuid),
            if position.is_some() {
                crate::combat::HIT_KNOCKBACK
            } else {
                0.0
            },
        ),
        None => (point, None, 0.0),
    };
    Some(PhysicalImpact {
        damage_hp: piece.amount,
        kind,
        source,
        origin,
        point,
        impulse,
        collision_radius: 0.0,
    })
}

pub(crate) fn impact_region(pose: DefensePose, point: [f64; 3]) -> Option<CoverageRegion> {
    if !pose_valid(pose) || !finite(point) {
        return None;
    }
    let yaw = f64::from(pose.yaw).to_radians();
    let dx = point[0] - pose.position[0];
    let dz = point[2] - pose.position[2];
    let side = dx * f64::from(mth::cos(yaw)) + dz * f64::from(mth::sin(yaw));
    let depth = -dx * f64::from(mth::sin(yaw)) + dz * f64::from(mth::cos(yaw));
    let height = (point[1] - pose.position[1]) / pose.body_height;
    if side.abs() > pose.half_width + 1.0e-6
        || depth.abs() > pose.half_width + 1.0e-6
        || !(0.0..=1.0).contains(&height)
    {
        return None;
    }
    Some(if height >= 0.8 {
        CoverageRegion::Head
    } else if height < 0.12 {
        CoverageRegion::Feet
    } else if height < 0.45 {
        CoverageRegion::Legs
    } else if side.abs() >= pose.half_width * 0.65 {
        CoverageRegion::Arms
    } else {
        CoverageRegion::Torso
    })
}

pub(crate) fn plan_physical_defense(
    pose: DefensePose,
    impact: PhysicalImpact,
    gear: &[DefenseGear<'_>],
    activation: Option<ShieldActivation>,
    stamina: f32,
    tick: u64,
) -> Option<DefensePlan> {
    if gear.is_empty()
        || !pose_valid(pose)
        || !impact_valid(impact)
        || !stamina.is_finite()
        || stamina < 0.0
    {
        return None;
    }
    let mut plan = DefensePlan {
        damage_hp: impact.damage_hp,
        stamina,
        armor_mass: 0.0,
        shield_blocked: false,
        shield_disabled_until: None,
        wear: [None; 6],
    };
    if let Some(active) = activation
        && active.slot < 2
        && let Some(item) = gear.iter().find(|item| item.slot == active.slot)
        && let Some(shield) = &item.profile.shield
        && shield.manual
        && item.damage < item.max_damage
        && shield_active(shield, active, tick)
        && shield.intersects_swept(
            pose.position,
            pose.yaw,
            impact.origin,
            impact.point,
            impact.collision_radius,
        )
    {
        if impact.kind == DamageKind::Hook {
            plan.shield_disabled_until =
                Some(tick.saturating_add(u64::from(shield.hook_disables_ticks)));
        }
        if impact.impulse <= shield.impulse_limit && plan.stamina >= shield.block_stamina_cost {
            plan.shield_blocked = true;
            plan.damage_hp = 0.0;
            plan.stamina -= shield.block_stamina_cost;
            plan.wear[item.slot] = wear(item, impact.damage_hp * shield.block_wear_per_damage_hp);
            return Some(plan);
        }
    }
    let region = impact_region(pose, impact.point);
    let mut reduction = 0.0_f32;
    let mut common_cap = 1.0_f32;
    for item in gear {
        let Some(armor) = &item.profile.armor else {
            continue;
        };
        if item.slot != armor_equipment_index(armor.slot)
            || item.max_damage == 0
            || item.damage >= item.max_damage
        {
            continue;
        }
        plan.armor_mass += armor.mass_units;
        let Some(region) = region else {
            continue;
        };
        let contribution = armor.reduction(
            region,
            impact.kind,
            (item.max_damage - item.damage) as f32 / item.max_damage as f32,
        );
        if contribution <= 0.0 {
            continue;
        }
        reduction += contribution;
        common_cap = common_cap.min(armor.common_cap);
        plan.stamina = (plan.stamina - armor.stamina_cost).max(0.0);
        plan.wear[item.slot] = wear(item, impact.damage_hp * armor.wear_per_damage_hp);
    }
    plan.damage_hp = impact.damage_hp * (1.0 - reduction.min(common_cap).clamp(0.0, 1.0));
    Some(plan)
}

/// Pure shared plan; NPC deployment owner alone applies custody and deadlines.
pub(crate) fn plan_deployed_shield_impact(
    shield: &ShieldProfile,
    pose: DeployedShieldPose,
    impact: PhysicalImpact,
    damage: u32,
    max_damage: u32,
    active: ShieldActivation,
    tick: u64,
) -> Option<DefensePlan> {
    if !shield.deployable
        || !finite(pose.position)
        || !pose.yaw.is_finite()
        || !impact_valid(impact)
        || max_damage == 0
        || damage >= max_damage
        || !shield_active(shield, active, tick)
        || !shield.intersects_swept(
            pose.position,
            pose.yaw,
            impact.origin,
            impact.point,
            impact.collision_radius,
        )
    {
        return None;
    }
    let blocked = impact.impulse <= shield.impulse_limit;
    let updated = damage.saturating_add(wear_amount(
        impact.damage_hp * shield.block_wear_per_damage_hp,
    ));
    let mut plan = DefensePlan {
        damage_hp: if blocked { 0.0 } else { impact.damage_hp },
        stamina: 0.0,
        armor_mass: 0.0,
        shield_blocked: blocked,
        shield_disabled_until: (impact.kind == DamageKind::Hook)
            .then(|| tick.saturating_add(u64::from(shield.hook_disables_ticks))),
        wear: [None; 6],
    };
    if updated != damage {
        plan.wear[0] = Some(DefenseWear {
            slot: 0,
            expected_damage: damage,
            updated_damage: updated.min(max_damage),
            broken: updated >= max_damage,
        });
    }
    Some(plan)
}

pub(crate) fn plan_deployed_item_damage(
    shield: &ShieldProfile,
    impact: PhysicalImpact,
    damage: u32,
    max_damage: u32,
) -> Option<DefenseWear> {
    if !impact_valid(impact) || max_damage == 0 || damage >= max_damage {
        return None;
    }
    let amount = wear_amount(impact.damage_hp * shield.block_wear_per_damage_hp);
    let updated = damage.saturating_add(amount);
    (amount > 0).then_some(DefenseWear {
        slot: 0,
        expected_damage: damage,
        updated_damage: updated.min(max_damage),
        broken: updated >= max_damage,
    })
}

fn shield_active(shield: &ShieldProfile, active: ShieldActivation, tick: u64) -> bool {
    tick >= active.disabled_until
        && tick >= active.started_tick
        && tick - active.started_tick >= u64::from(shield.activation_ticks)
}
fn wear(item: &DefenseGear<'_>, amount: f32) -> Option<DefenseWear> {
    let amount = wear_amount(amount);
    (amount > 0).then(|| {
        let updated = item.damage.saturating_add(amount);
        DefenseWear {
            slot: item.slot,
            expected_damage: item.damage,
            updated_damage: updated.min(item.max_damage),
            broken: updated >= item.max_damage,
        }
    })
}
fn wear_amount(amount: f32) -> u32 {
    amount.ceil().clamp(0.0, u32::MAX as f32) as u32
}
fn finite(point: [f64; 3]) -> bool {
    point.into_iter().all(f64::is_finite)
}
fn pose_valid(pose: DefensePose) -> bool {
    finite(pose.position)
        && pose.yaw.is_finite()
        && pose.body_height.is_finite()
        && pose.body_height > 0.0
        && pose.half_width.is_finite()
        && pose.half_width > 0.0
}
fn impact_valid(impact: PhysicalImpact) -> bool {
    impact.damage_hp.is_finite()
        && impact.damage_hp > 0.0
        && finite(impact.origin)
        && finite(impact.point)
        && impact.impulse.is_finite()
        && impact.impulse >= 0.0
        && impact.collision_radius.is_finite()
        && impact.collision_radius >= 0.0
}

#[cfg(test)]
#[path = "physical_defense_tests.rs"]
mod tests;
