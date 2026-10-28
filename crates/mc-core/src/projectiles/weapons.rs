//! Player-owned item charges and atomic ammunition custody. Flight is shared
//! with mob and dispenser projectiles; no connection owns an in-flight item.

use mc_physics::mth;
use std::sync::Arc;

use mc_data::ItemStack;
use mc_data::statistics::StatType;

use super::{ProjectileKind, ProjectileState};
use crate::boundary::{EntityDataValue, EntityId, Hand, PlayerChange};
use crate::enchantments::level;
use crate::inventory::OFF_HAND;
use crate::tick::TickState;

#[cfg(test)]
#[path = "weapons_charge_tests.rs"]
mod charge_tests;
#[cfg(test)]
#[path = "weapons_floor_tests.rs"]
pub(super) mod floor_tests;
#[cfg(test)]
#[path = "weapons_tests.rs"]
mod tests;

#[derive(Debug)]
pub struct WeaponUse {
    hand: Hand,
    slot: usize,
    stack: ItemStack,
    started: u64,
}

#[derive(Debug)]
pub(crate) struct Riptide {
    pub until: u64,
    pub previous: [f64; 3],
    pub hit: Vec<uuid::Uuid>,
    pub stack: ItemStack,
}

pub(crate) fn held_slot(state: &TickState, actor: EntityId, hand: Hand) -> Option<usize> {
    let player = state.players.get(&actor)?;
    Some(match hand {
        Hand::Main => usize::from(player.inventory.selected.min(8)),
        Hand::Off => OFF_HAND,
    })
}

/// Vanilla ThrowableProjectile/Snowball launch: eye-height origin at the
/// shooter's eye minus 0.1, inherited shooter motion (Y only while airborne),
/// and a -20deg pitch bias for thrown potions (ThrowablePotionItem).
pub(crate) fn geometry(
    state: &TickState,
    actor: EntityId,
    speed: f64,
) -> Option<([f64; 3], [f64; 3])> {
    geometry_for(state, actor, speed, false)
}

pub(crate) fn geometry_for(
    state: &TickState,
    actor: EntityId,
    speed: f64,
    potion: bool,
) -> Option<([f64; 3], [f64; 3])> {
    let slot = state.entities.slot_of(actor)?;
    let pos = state.entities.pos(slot);
    let rot = state.entities.rot(slot);
    // The mth tables map any angle (even infinity) to a finite value; refuse it here.
    if !rot[0].is_finite() || !rot[1].is_finite() {
        return None;
    }
    let pitch_deg = f64::from(rot[1]) + if potion { -20.0 } else { 0.0 };
    let (yaw, pitch) = (f64::from(rot[0]).to_radians(), pitch_deg.to_radians());
    let direction = [
        -f64::from(mth::sin(yaw)) * f64::from(mth::cos(pitch)),
        -f64::from(mth::sin(pitch)),
        f64::from(mth::cos(yaw)) * f64::from(mth::cos(pitch)),
    ];
    // Vanilla eye origin (eyeY - 0.1); pose-aware height, not a fixed 1.52.
    let eye =
        pos[1] + crate::tick::player_eye_height(state.entities.row(slot).appearance.pose) - 0.1;
    let origin = [pos[0], eye, pos[2]];
    let mut velocity = direction.map(|v| v * speed);
    // Inherit shooter motion: full XZ drift while grounded, Y only airborne.
    let motion = state.entities.vel(slot).map(|v| v / 20.0);
    let airborne = !state.entities.ground(slot);
    velocity[0] += motion[0];
    velocity[2] += motion[2];
    if airborne {
        velocity[1] += motion[1];
    }
    Some((origin, velocity))
}

pub(crate) fn sync(state: &mut TickState, actor: EntityId) {
    if let Some(player) = state.players.get(&actor) {
        let connection = player.connection;
        let change = player.inventory_contents();
        state.push_player(connection, change);
    }
}

/// The native Unbreaking roll is made once against a local seed. Admission
/// observes count loss; ordinary component wear never spends a resource hold.
pub(crate) struct WearPlan {
    damage: Option<i32>,
    broken: bool,
    seed: i32,
}

pub(crate) fn plan_wear(
    state: &TickState,
    actor: EntityId,
    slot: usize,
    amount: i32,
) -> Option<WearPlan> {
    let player = state.players.get(&actor)?;
    if player.creative() || player.spectator() || amount <= 0 {
        return None;
    }
    let stack = &player.inventory.slots[slot];
    let rules = state.registry.item_rules();
    let facts = rules.facts.facts_for_stack(stack, rules.items)?;
    let maximum = facts.max_damage?;
    let mut seed = player.enchantment_seed;
    let amount =
        crate::enchantments::durability_damage(stack, amount, facts.armor.is_some(), &mut seed);
    let damage = (amount > 0).then(|| stack.damage.unwrap_or(0).saturating_add(amount));
    Some(WearPlan {
        damage,
        broken: damage
            .is_some_and(|damage| u32::try_from(damage).is_ok_and(|damage| damage >= maximum)),
        seed,
    })
}

pub(crate) fn costs_allowed(
    state: &TickState,
    actor: EntityId,
    debits: &[(usize, u32)],
    weapon_slot: usize,
    wear: Option<&WearPlan>,
) -> bool {
    !crate::plugin_inventories::below_player_debit_floors(
        state,
        actor,
        debits,
        wear.filter(|plan| plan.broken).map(|_| weapon_slot),
    )
}

pub(crate) fn apply_wear(
    state: &mut TickState,
    actor: EntityId,
    slot: usize,
    plan: Option<WearPlan>,
) {
    let Some(plan) = plan else {
        return;
    };
    let observed = plan.damage.is_some()
        && !state.registry.advancements.definitions.is_empty()
        && (state.registry.advancements.trigger_index.is_empty()
            || state
                .registry
                .advancements
                .trigger_index
                .contains_key("minecraft:item_durability_changed"));
    let before = observed.then(|| state.players[&actor].inventory.slots[slot].clone());
    let player = state.players.get_mut(&actor).unwrap();
    player.enchantment_seed = plan.seed;
    if plan.broken {
        let item_id = player.inventory.slots[slot].item_id;
        player.inventory.slots[slot] = ItemStack::EMPTY;
        player.statistics.increment(StatType::Broken, item_id, 1);
    } else if let Some(damage) = plan.damage {
        player.inventory.slots[slot].damage = Some(damage);
    }
    if let Some(before) = before {
        let damage = plan.damage.expect("observed durability change");
        let previous_damage = before.damage.unwrap_or(0);
        let rules = state.registry.item_rules();
        let max_damage = rules
            .facts
            .facts_for_stack(&before, rules.items)
            .and_then(|facts| facts.max_damage)
            .expect("planned damageable item");
        let item = crate::advancements::item_snapshot(state, &before);
        crate::advancements::trigger(
            state,
            actor,
            "minecraft:item_durability_changed",
            serde_json::json!({
                "item": item,
                "durability": i64::from(max_damage) - i64::from(damage),
                "delta": previous_damage.saturating_sub(damage),
            }),
        );
    }
}

pub(crate) fn wear(state: &mut TickState, actor: EntityId, slot: usize, amount: i32) {
    let plan = plan_wear(state, actor, slot, amount);
    if costs_allowed(state, actor, &[], slot, plan.as_ref()) {
        apply_wear(state, actor, slot, plan);
    }
}

fn consume(stack: &mut ItemStack) {
    mc_data::inventory_semantics_26_1_2::decrement_stack(stack);
}

fn use_flags(state: &mut TickState, actor: EntityId, flags: i8) {
    let Some(slot) = state.entities.slot_of(actor) else {
        return;
    };
    let extra = &mut Arc::make_mut(&mut state.entities.row_mut(slot).appearance).extra;
    if let Some((_, value)) = extra.iter_mut().find(|(index, _)| *index == 8) {
        *value = EntityDataValue::Byte(flags);
    } else {
        extra.push((8, EntityDataValue::Byte(flags)));
    }
    super::touch(state, actor);
}

fn ammo(state: &TickState, actor: EntityId, weapon_slot: usize, crossbow: bool) -> Option<usize> {
    let player = state.players.get(&actor)?;
    let selected = usize::from(player.inventory.selected.min(8));
    [OFF_HAND, selected].into_iter().chain(0..36).find(|&slot| {
        if slot == weapon_slot {
            return false;
        }
        let stack = &player.inventory.slots[slot];
        !stack.is_empty()
            && super::item_name(state, stack).is_some_and(|name| {
                matches!(
                    name,
                    "minecraft:arrow" | "minecraft:spectral_arrow" | "minecraft:tipped_arrow"
                ) || (crossbow
                    && name == "minecraft:firework_rocket"
                    && (slot == selected || slot == OFF_HAND))
            })
    })
}

fn ammo_stack(
    state: &TickState,
    actor: EntityId,
    weapon_slot: usize,
    crossbow: bool,
) -> Option<(Option<usize>, ItemStack)> {
    if let Some(slot) = ammo(state, actor, weapon_slot, crossbow) {
        let mut stack = state.players[&actor].inventory.slots[slot].clone();
        stack.count = 1;
        return Some((Some(slot), stack));
    }
    state
        .players
        .get(&actor)
        .filter(|player| player.creative())?;
    let arrow = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:arrow").ok()?)?;
    Some((None, ItemStack::new(arrow, 1)))
}

fn available(state: &TickState, kind: ProjectileKind) -> bool {
    state
        .registry
        .blueprints
        .iter()
        .any(|(_, blueprint)| blueprint.name == kind.name())
}

fn arrow_kind(state: &TickState, stack: &ItemStack) -> ProjectileKind {
    if super::item_name(state, stack) == Some("minecraft:spectral_arrow") {
        ProjectileKind::SpectralArrow
    } else {
        ProjectileKind::Arrow
    }
}

/// Launch a detached firework from the clicked block face.
pub(crate) fn use_rocket_on_block(
    state: &mut TickState,
    actor: EntityId,
    hand: Hand,
    position: crate::boundary::BlockPos,
    face: crate::placement::Face,
    cursor: [f32; 3],
) -> bool {
    let Some(slot) = held_slot(state, actor, hand) else {
        return false;
    };
    let stack = state.players[&actor].inventory.slots[slot].clone();
    if stack.is_empty() || super::item_name(state, &stack) != Some("minecraft:firework_rocket") {
        return false;
    }
    let player = &state.players[&actor];
    if player.spectator()
        || player.vitals.is_dead()
        || player.item_on_cooldown(stack.item_id)
        || !available(state, ProjectileKind::Firework)
        || (!player.creative() && !costs_allowed(state, actor, &[(slot, 1)], slot, None))
    {
        return true;
    }
    let Some(actor_slot) = state.entities.slot_of(actor) else {
        return true;
    };
    let owner = state.entities.row(actor_slot).uuid;
    let offset = face.offset();
    let block = [position.x, position.y, position.z];
    let origin = std::array::from_fn(|i| {
        f64::from(block[i]) + f64::from(cursor[i]) + f64::from(offset[i]) * 0.15
    });
    let item_id = stack.item_id;
    let record = ProjectileState::new(ProjectileKind::Firework, Some(owner), stack);
    if super::launch(state, record, origin, [0.0, 0.05, 0.0]).is_none() {
        return true;
    }
    let player = state.players.get_mut(&actor).unwrap();
    player.statistics.increment(StatType::Used, item_id, 1);
    if !player.creative() {
        consume(&mut player.inventory.slots[slot]);
    }
    sync(state, actor);
    true
}

/// Returns true for a projectile item even when its use is refused. Such an
/// item must not fall through into food consumption or other action owners.
pub(crate) fn use_item(state: &mut TickState, actor: EntityId, hand: Hand) -> bool {
    let Some(slot) = held_slot(state, actor, hand) else {
        return false;
    };
    let stack = state.players[&actor].inventory.slots[slot].clone();
    if stack.is_empty() {
        return false;
    }
    let name = super::item_name(state, &stack).unwrap_or("");
    if name == "minecraft:ender_eye" {
        super::eye::use_item(state, actor, hand);
        return true;
    }
    let kind = match name {
        "minecraft:snowball" => Some(ProjectileKind::Snowball),
        // Vanilla throws blue/brown eggs as ordinary egg projectiles.
        "minecraft:egg" | "minecraft:blue_egg" | "minecraft:brown_egg" => Some(ProjectileKind::Egg),
        "minecraft:ender_pearl" => Some(ProjectileKind::EnderPearl),
        "minecraft:splash_potion" => Some(ProjectileKind::Potion),
        "minecraft:lingering_potion" => Some(ProjectileKind::LingeringPotion),
        "minecraft:experience_bottle" => Some(ProjectileKind::ExperienceBottle),
        "minecraft:firework_rocket" => Some(ProjectileKind::Firework),
        "minecraft:wind_charge" => Some(ProjectileKind::WindCharge),
        _ => None,
    };
    let charged = matches!(
        name,
        "minecraft:bow" | "minecraft:crossbow" | "minecraft:trident"
    );
    if kind.is_none() && !charged && name != "minecraft:fishing_rod" {
        return false;
    }
    let player = &state.players[&actor];
    // Vanilla FireworkRocketItem.use passes without spawning or consuming
    // anything unless the player is gliding; block use is a separate path.
    if kind == Some(ProjectileKind::Firework) && !player.fall_flying {
        return true;
    }
    if player.spectator() || player.vitals.is_dead() || player.item_on_cooldown(stack.item_id) {
        return true;
    }
    if name == "minecraft:fishing_rod" {
        return super::fishing::use_item(state, actor, hand);
    }
    if name == "minecraft:crossbow" && !stack.extended().charged_projectiles.is_empty() {
        fire_crossbow(state, actor, slot, &stack);
        return true;
    }
    if charged {
        // LivingEntity.startUsingItem ignores another start while the current
        // use is active. Replacing the clock here makes a late native use
        // packet turn an already-drawn weapon into a too-short release.
        if state
            .projectiles
            .using
            .get(&actor)
            .is_some_and(|using| valid_use(state, actor, using))
        {
            return true;
        }
        if name == "minecraft:trident" && stack.damage.unwrap_or(0) >= 249 {
            return true;
        }
        if name != "minecraft:trident" {
            let Some((ammo_slot, ammo)) =
                ammo_stack(state, actor, slot, name == "minecraft:crossbow")
            else {
                return true;
            };
            let free = player.creative()
                || (name == "minecraft:bow"
                    && level(&stack, "infinity") > 0
                    && super::item_name(state, &ammo) == Some("minecraft:arrow"));
            if !free
                && ammo_slot.is_some_and(|ammo_slot| {
                    !costs_allowed(state, actor, &[(ammo_slot, 1)], slot, None)
                })
            {
                return true;
            }
        }
        if name == "minecraft:trident"
            && level(&stack, "riptide") == 0
            && !player.creative()
            && !costs_allowed(state, actor, &[(slot, 1)], slot, None)
        {
            return true;
        }
        if name == "minecraft:trident" && level(&stack, "riptide") > 0 && !wet(state, actor) {
            return true;
        }
        state.players.get_mut(&actor).unwrap().release_use_item();
        state.projectiles.using.insert(
            actor,
            WeaponUse {
                hand,
                slot,
                stack,
                started: state.tick,
            },
        );
        use_flags(state, actor, if hand == Hand::Off { 3 } else { 1 });
        return true;
    }
    let kind = kind.expect("admitted projectile");
    if !available(state, kind) {
        return true;
    }
    let speed = match kind {
        ProjectileKind::Potion | ProjectileKind::LingeringPotion => 0.5,
        ProjectileKind::ExperienceBottle => 0.7,
        ProjectileKind::Firework => 0.05,
        _ => 1.5,
    };
    let potion_throw = matches!(
        kind,
        ProjectileKind::Potion | ProjectileKind::LingeringPotion
    );
    let Some((origin, mut velocity)) = geometry_for(state, actor, speed, potion_throw) else {
        return true;
    };
    if kind == ProjectileKind::Firework {
        velocity = [0.0, 0.05, 0.0];
    }
    if !player.creative() && !costs_allowed(state, actor, &[(slot, 1)], slot, None) {
        return true;
    }
    let owner = state
        .entities
        .row(state.entities.slot_of(actor).unwrap())
        .uuid;
    let mut record = ProjectileState::new(kind, Some(owner), stack.clone());
    if kind == ProjectileKind::Firework {
        record.attached_to = Some(owner);
    }
    if super::launch(state, record, origin, velocity).is_none() {
        return true;
    }
    let rules = state.registry.item_rules();
    let player = state.players.get_mut(&actor).unwrap();
    player
        .statistics
        .increment(StatType::Used, stack.item_id, 1);
    if !player.creative() {
        consume(&mut player.inventory.slots[slot]);
    }
    let ticks = match kind {
        ProjectileKind::EnderPearl => 20,
        ProjectileKind::WindCharge => 10,
        _ => 0,
    };
    let change = if ticks > 0 {
        player.start_item_cooldown(&rules, stack.item_id, ticks)
    } else {
        None
    };
    if let Some(change) = change {
        let connection = state.players[&actor].connection;
        state.push_player(connection, change);
    }
    sync(state, actor);
    true
}

pub(crate) fn cancel_main(state: &mut TickState, actor: EntityId) {
    if state
        .projectiles
        .using
        .get(&actor)
        .is_some_and(|using| using.hand == Hand::Main)
    {
        cancel(state, actor);
    }
}

pub(crate) fn cancel(state: &mut TickState, actor: EntityId) {
    if state.projectiles.using.remove(&actor).is_some() {
        use_flags(state, actor, 0);
    }
}

pub(crate) fn release(state: &mut TickState, actor: EntityId) -> bool {
    let Some(using) = state.projectiles.using.remove(&actor) else {
        return false;
    };
    if !valid_use(state, actor, &using) {
        use_flags(state, actor, 0);
        return false;
    }
    let elapsed = state.tick.saturating_sub(using.started);
    let admitted = match super::item_name(state, &using.stack) {
        Some("minecraft:bow") => bow(state, actor, &using, elapsed),
        Some("minecraft:crossbow") if elapsed >= crossbow_ticks(&using.stack) => {
            load_crossbow(state, actor, &using)
        }
        Some("minecraft:trident") if elapsed >= 10 => trident(state, actor, &using),
        _ => true,
    };
    if admitted {
        let spinning = super::item_name(state, &using.stack) == Some("minecraft:trident")
            && level(&using.stack, "riptide") > 0
            && state.projectiles.riptides.contains_key(&actor);
        use_flags(state, actor, if spinning { 4 } else { 0 });
    } else {
        // A resource refusal does not advance the charge clock or publish an
        // equipment/use transition. Release can succeed after the hold ends.
        state.projectiles.using.insert(actor, using);
    }
    true
}

fn valid_use(state: &TickState, actor: EntityId, using: &WeaponUse) -> bool {
    state.entities.slot_of(actor).is_some()
        && state.players.get(&actor).is_some_and(|player| {
            !player.spectator()
                && !player.vitals.is_dead()
                && held_slot(state, actor, using.hand) == Some(using.slot)
                && player.inventory.slots[using.slot] == using.stack
        })
}

fn crossbow_ticks(stack: &ItemStack) -> u64 {
    25_u64
        .saturating_sub(u64::from(level(stack, "quick_charge")) * 5)
        .max(1)
}

fn bow(state: &mut TickState, actor: EntityId, using: &WeaponUse, elapsed: u64) -> bool {
    let charge = (elapsed as f64 / 20.0).min(1.0);
    let power = ((charge * charge + 2.0 * charge) / 3.0).min(1.0);
    if power < 0.1 {
        return true;
    }
    let Some((ammo_slot, stack)) = ammo_stack(state, actor, using.slot, false) else {
        return true;
    };
    let kind = arrow_kind(state, &stack);
    if !available(state, kind) {
        return true;
    }
    let free = state.players[&actor].creative()
        || (level(&using.stack, "infinity") > 0
            && super::item_name(state, &stack) == Some("minecraft:arrow"));
    let Some((origin, velocity)) = geometry(state, actor, power * 3.0) else {
        return true;
    };
    let wear = plan_wear(state, actor, using.slot, 1);
    let debit = ammo_slot.filter(|_| !free).map(|slot| (slot, 1));
    if !costs_allowed(state, actor, debit.as_slice(), using.slot, wear.as_ref()) {
        return false;
    }
    let owner = state
        .entities
        .row(state.entities.slot_of(actor).unwrap())
        .uuid;
    let mut record = ProjectileState::new(kind, Some(owner), stack);
    record.pickup = !free;
    record.critical = power >= 1.0;
    let level = level(&using.stack, "power");
    if level > 0 {
        record.damage += level as f32 * 0.5 + 0.5;
    }
    record.punch = crate::enchantments::level(&using.stack, "punch");
    record.flame = crate::enchantments::level(&using.stack, "flame") > 0;
    if super::launch(state, record, origin, velocity).is_none() {
        return true;
    }
    if let Some((slot, _)) = debit {
        consume(&mut state.players.get_mut(&actor).unwrap().inventory.slots[slot]);
    }
    state.players.get_mut(&actor).unwrap().statistics.increment(
        StatType::Used,
        using.stack.item_id,
        1,
    );
    apply_wear(state, actor, using.slot, wear);
    sync(state, actor);
    true
}

fn load_crossbow(state: &mut TickState, actor: EntityId, using: &WeaponUse) -> bool {
    let Some((ammo_slot, stack)) = ammo_stack(state, actor, using.slot, true) else {
        return true;
    };
    let debit = ammo_slot
        .filter(|_| !state.players[&actor].creative())
        .map(|slot| (slot, 1));
    if !costs_allowed(state, actor, debit.as_slice(), using.slot, None) {
        return false;
    }
    let count = if level(&using.stack, "multishot") > 0 {
        3
    } else {
        1
    };
    let player = state.players.get_mut(&actor).unwrap();
    player.inventory.slots[using.slot]
        .extended_mut()
        .charged_projectiles = vec![stack; count];
    if let Some((slot, _)) = debit {
        consume(&mut player.inventory.slots[slot]);
    }
    sync(state, actor);
    true
}

fn rollback_crossbow_launches(state: &mut TickState, spawned: &[Option<EntityId>]) {
    for id in spawned.iter().flatten() {
        if let Some(slot) = state.entities.slot_of_any(*id) {
            state.remove_entity(slot);
        }
    }
    // These entities never published. Roll back their queued births/appearance
    // too, rather than pairing nonexistent births with durable tombstones.
    state
        .projectiles
        .spawned
        .retain(|id| !spawned.contains(&Some(*id)));
    state
        .appearance_changed
        .retain(|id| !spawned.contains(&Some(*id)));
}
fn fire_crossbow(state: &mut TickState, actor: EntityId, slot: usize, weapon: &ItemStack) {
    let ammo = &weapon.extended().charged_projectiles;
    if ammo.is_empty()
        || ammo.len() > 3
        || ammo
            .iter()
            .any(|stack| stack.is_empty() || stack.count != 1)
    {
        return;
    }
    let mut kinds = [ProjectileKind::Arrow; 3];
    for (index, stack) in ammo.iter().enumerate() {
        let kind = match super::item_name(state, stack) {
            Some("minecraft:firework_rocket") => ProjectileKind::Firework,
            Some("minecraft:arrow" | "minecraft:tipped_arrow" | "minecraft:spectral_arrow") => {
                arrow_kind(state, stack)
            }
            _ => return,
        };
        if !available(state, kind) {
            return;
        }
        kinds[index] = kind;
    }
    let Some(owner_slot) = state.entities.slot_of(actor) else {
        return;
    };
    let owner = state.entities.row(owner_slot).uuid;
    let creative = state.players[&actor].creative();
    // Validate the common geometry before spawning any member of a multishot.
    // Unit direction, finite origin, and the bounded speeds/angles below ensure
    // every member is finite before custody or the entity store is mutated.
    let Some((origin, direction)) = geometry(state, actor, 1.0) else {
        return;
    };
    if origin
        .iter()
        .chain(direction.iter())
        .any(|value| !value.is_finite())
    {
        return;
    }
    let wear_amount = kinds[..ammo.len()]
        .iter()
        .map(|kind| {
            if *kind == ProjectileKind::Firework {
                3
            } else {
                1
            }
        })
        .sum();
    let wear = plan_wear(state, actor, slot, wear_amount);
    if !costs_allowed(state, actor, &[], slot, wear.as_ref()) {
        return;
    }
    let mut spawned = [None; 3];
    let shared_weapon = Arc::new(weapon.clone());
    for (index, (stack, kind)) in ammo.iter().zip(kinds).enumerate() {
        let speed = if kind == ProjectileKind::Firework {
            1.6
        } else {
            3.15
        };
        let velocity = direction.map(|value| value * speed);
        let angle: f64 = match index {
            1 => -10.0,
            2 => 10.0,
            _ => 0.0,
        };
        let radians = angle.to_radians();
        let (sin, cos) = (f64::from(mth::sin(radians)), f64::from(mth::cos(radians)));
        let velocity = [
            velocity[0] * cos - velocity[2] * sin,
            velocity[1],
            velocity[0] * sin + velocity[2] * cos,
        ];
        let mut record = ProjectileState::new(kind, Some(owner), stack.clone());
        record.pickup = !creative && index == 0;
        record.piercing = level(weapon, "piercing").min(127);
        record.critical = kind.arrow();
        record.shot_at_angle = kind == ProjectileKind::Firework;
        record.fired_from_weapon = Some(Arc::clone(&shared_weapon));
        let Some(id) = super::launch(state, record, origin, velocity) else {
            rollback_crossbow_launches(state, &spawned[..index]);
            return;
        };
        spawned[index] = Some(id);
    }
    state
        .players
        .get_mut(&actor)
        .unwrap()
        .statistics
        .increment(StatType::Used, weapon.item_id, 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[slot]
        .extended_mut()
        .charged_projectiles
        .clear();
    apply_wear(state, actor, slot, wear);
    sync(state, actor);
    let item = crate::advancements::item_snapshot(state, weapon);
    crate::advancements::trigger(
        state,
        actor,
        "minecraft:shot_crossbow",
        serde_json::json!({ "item": item }),
    );
}

fn wet(state: &TickState, actor: EntityId) -> bool {
    let Some(slot) = state.entities.slot_of(actor) else {
        return false;
    };
    let pos = state.entities.pos(slot);
    let water = state
        .world
        .block_at(
            pos[0].floor() as i32,
            pos[1].floor() as i32,
            pos[2].floor() as i32,
        )
        .is_some_and(|block| {
            state.registry.materials.classify(block) == mc_physics::BlockMaterial::Water
        });
    water
        || (state.dimension.has_weather()
            && state.weather.0 != crate::commands::Weather::Clear
            && ((pos[1] + 2.0).floor() as i32..state.dimension.geometry().max_y()).all(|y| {
                state
                    .world
                    .block_at(pos[0].floor() as i32, y, pos[2].floor() as i32)
                    .is_some_and(|block| {
                        state.registry.materials.classify(block) != mc_physics::BlockMaterial::Solid
                    })
            }))
}

fn trident(state: &mut TickState, actor: EntityId, using: &WeaponUse) -> bool {
    let riptide = level(&using.stack, "riptide").min(3);
    if riptide > 0 {
        if !wet(state, actor) {
            return true;
        }
        let Some((_, velocity)) = geometry(state, actor, 3.0 * f64::from(1 + riptide) / 4.0) else {
            return true;
        };
        let wear = plan_wear(state, actor, using.slot, 1);
        if !costs_allowed(state, actor, &[], using.slot, wear.as_ref()) {
            return false;
        }
        let slot = state.entities.slot_of(actor).unwrap();
        let old = state.entities.vel(slot);
        let velocity = std::array::from_fn(|axis| old[axis] + velocity[axis] * 20.0);
        state.entities.set_vel(slot, velocity);
        let connection = state.players[&actor].connection;
        state.push_player(
            connection,
            PlayerChange::Motion {
                entity: actor,
                velocity,
            },
        );
        let position = state.entities.pos(slot);
        state.projectiles.riptides.insert(
            actor,
            Riptide {
                until: state.tick.saturating_add(20),
                previous: position,
                hit: Vec::new(),
                stack: using.stack.clone(),
            },
        );
        use_flags(state, actor, 4);
        state.players.get_mut(&actor).unwrap().statistics.increment(
            StatType::Used,
            using.stack.item_id,
            1,
        );
        apply_wear(state, actor, using.slot, wear);
        sync(state, actor);
        return true;
    }
    if !available(state, ProjectileKind::Trident) {
        return true;
    }
    let Some((origin, velocity)) = geometry(state, actor, 2.5) else {
        return true;
    };
    let owner = state
        .entities
        .row(state.entities.slot_of(actor).unwrap())
        .uuid;
    let creative = state.players[&actor].creative();
    let mut seed = state.players[&actor].enchantment_seed;
    // Throwing moves one retained stack rather than breaking it in the source.
    // Preserve the native trident's damage component, including Some(0) after
    // a successful Unbreaking skip on a previously undamaged stack.
    let damage = (!creative).then(|| {
        using
            .stack
            .damage
            .unwrap_or(0)
            .saturating_add(crate::enchantments::durability_damage(
                &using.stack,
                1,
                false,
                &mut seed,
            ))
    });
    let debit = (!creative).then_some((using.slot, 1));
    if !costs_allowed(state, actor, debit.as_slice(), using.slot, None) {
        return false;
    }
    let mut stack = using.stack.clone();
    stack.count = 1;
    if let Some(damage) = damage {
        stack.damage = Some(damage);
    }
    let mut record = ProjectileState::new(ProjectileKind::Trident, Some(owner), stack);
    record.pickup = !creative;
    if super::launch(state, record, origin, velocity).is_some() {
        let player = state.players.get_mut(&actor).unwrap();
        player
            .statistics
            .increment(StatType::Used, using.stack.item_id, 1);
        if !creative {
            player.enchantment_seed = seed;
            consume(&mut player.inventory.slots[using.slot]);
        }
    }
    sync(state, actor);
    true
}

pub(crate) fn tick(state: &mut TickState, intents: &mut Vec<crate::intent::Intent>) {
    let mut charges = std::mem::take(&mut state.projectiles.using);
    charges.retain(|actor, using| {
        let actor = *actor;
        if !valid_use(state, actor, using) {
            use_flags(state, actor, 0);
            return false;
        }
        if super::item_name(state, &using.stack) == Some("minecraft:crossbow")
            && state.tick.saturating_sub(using.started) >= crossbow_ticks(&using.stack)
        {
            if !load_crossbow(state, actor, using) {
                return true;
            }
            use_flags(state, actor, 0);
            false
        } else {
            true
        }
    });
    state.projectiles.using = charges;
    let mut riptides = std::mem::take(&mut state.projectiles.riptides);
    riptides.retain(|actor, spin| {
        let actor = *actor;
        let Some(slot) = state.entities.slot_of(actor) else {
            return false;
        };
        if state.tick >= spin.until
            || state
                .players
                .get(&actor)
                .is_none_or(|p| p.spectator() || p.vitals.is_dead())
        {
            use_flags(state, actor, 0);
            return false;
        }
        let position = state.entities.pos(slot);
        let step = std::array::from_fn(|axis| position[axis] - spin.previous[axis]);
        for index in 0..state.entities.len() as u32 {
            let target_slot = crate::entities::EntitySlot(index);
            let row = state.entities.row(target_slot);
            if !state.entities.body(target_slot).alive
                || state.entities.body(target_slot).id == actor
                || state.entities.body(target_slot).movement
                    == crate::blueprint::MovementClass::Projectile
                || spin.hit.contains(&row.uuid)
            {
                continue;
            }
            if !matches!(
                state.entities.body(target_slot).kind,
                crate::entities::EntityKind::Mob { .. }
                    | crate::entities::EntityKind::Player { .. }
            ) {
                continue;
            }
            let target = state.entities.body(target_slot).id;
            let uuid = row.uuid;
            let at = state.entities.pos(target_slot);
            let min = [
                at[0] - state.entities.body(target_slot).half_width - 0.5,
                at[1] - 1.8,
                at[2] - state.entities.body(target_slot).half_width - 0.5,
            ];
            let max = [
                at[0] + state.entities.body(target_slot).half_width + 0.5,
                at[1] + state.entities.body(target_slot).height,
                at[2] + state.entities.body(target_slot).half_width + 0.5,
            ];
            if super::flight::segment_enters(spin.previous, step, min, max).is_some() {
                let seq = state.next_seq(actor);
                intents.push(crate::intent::Intent {
                    actor,
                    seq,
                    kind: crate::intent::IntentKind::DamageEntity {
                        target,
                        amount: 9.0
                            + crate::enchantments::target_damage(state, target, &spin.stack),
                        kind: crate::survival::DamageKind::Melee,
                        source: Some(crate::intent::DamageSource::Melee {
                            attacker: actor,
                            position: spin.previous,
                            yaw: state.entities.rot(slot)[0],
                            bonus_knockback: 0.0,
                            sweep: false,
                            fire_ticks: 0,
                            bane_ticks: 0,
                        }),
                    },
                    footprint: crate::intent::Footprint {
                        entity_writes: vec![target],
                        ..Default::default()
                    },
                });
                spin.hit.push(uuid);
            }
        }
        spin.previous = position;
        true
    });
    state.projectiles.riptides = riptides;
}
