//! Retained UUID/fence leash anchors with snapshot-convergent links.
use crate::boundary::{BlockPos, ChangeSet, EntityId};
use crate::entities::EntitySlot;
use crate::tick::TickState;
use mc_data::{Identifier, ItemStack};
use mc_physics::mth;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Holder {
    Entity(Uuid),
    Fence([i32; 3]),
}
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Attach,
    Detach,
    Name,
}
pub(crate) fn action(
    row: &crate::entities::ColdRow,
    actor: Uuid,
    leashable: bool,
    held: &str,
    stack: &ItemStack,
) -> Option<Action> {
    if held == "minecraft:name_tag" {
        return stack.custom_name.as_ref().map(|_| Action::Name);
    }
    if row.entity_life.leash == Some(Holder::Entity(actor)) {
        return Some(Action::Detach);
    }
    if held == "minecraft:lead" && leashable && row.entity_life.leash.is_none() {
        return Some(Action::Attach);
    }
    None
}
pub(crate) fn apply(
    state: &mut TickState,
    slot: EntitySlot,
    actor: Uuid,
    action: &Action,
    held: &ItemStack,
) {
    match action {
        Action::Attach => state.entities.set_leash(slot, Some(Holder::Entity(actor))),
        Action::Detach => detach(state, slot),
        Action::Name => {
            let row = state.entities.row_mut(slot);
            std::sync::Arc::make_mut(&mut row.appearance).custom_name = Some(
                held.custom_name
                    .as_deref()
                    .expect("validated name tag")
                    .to_owned(),
            );
            row.persistent = true;
        }
    }
}
pub(crate) fn detach(state: &mut TickState, slot: EntitySlot) {
    let was_leashed = state.entities.row(slot).entity_life.leash.is_some();
    state.entities.set_leash(slot, None);
    if was_leashed {
        if let Some(id) = state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:lead").expect("static id"))
        {
            state
                .merge_life_drops
                .push((state.entities.pos(slot), ItemStack::new(id, 1)));
        }
        std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).leash_holder = None;
        state.appearance_changed.push(state.entities.body(slot).id);
    }
}
pub(crate) fn initialize(state: &mut TickState, slot: EntitySlot) {
    if crate::mounts::name(state, slot) != "minecraft:leash_knot" {
        return;
    }
    static ID: std::sync::LazyLock<Identifier> = std::sync::LazyLock::new(|| {
        Identifier::parse("minecraft:leash_knot").expect("static entity id")
    });
    let dimensions = state
        .registry
        .entity_types
        .facts_of(&ID)
        .expect("required leash knot contract")
        .dimensions;
    state.entities.set_half_width(slot, dimensions.half_width());
    let (body, row) = state.entities.parts_mut(slot);
    body.movement = crate::blueprint::MovementClass::Stationary;
    body.height = dimensions.height;
    row.speed = 0.0;
    row.attack_damage = 0.0;
    state.entities.set_vel(slot, [0.0; 3]);
    let position = state.entities.pos(slot);
    state.entities.set_pos(
        slot,
        [
            position[0].floor() + 0.5,
            position[1].floor() + 0.375,
            position[2].floor() + 0.5,
        ],
    );
}
fn fence(state: &TickState, pos: [i32; 3]) -> bool {
    state
        .world
        .block_at(pos[0], pos[1], pos[2])
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|s| s.block.id.as_str().ends_with("_fence"))
}
/// Read-only dispatch for an off-tick build preview: tying a held mob owns
/// this click before any ordinary adjacent block placement can happen.
pub(crate) fn would_use_fence(state: &TickState, actor: EntityId, pos: BlockPos) -> bool {
    if state
        .players
        .get(&actor)
        .is_none_or(|player| player.spectator() || player.vitals.is_dead())
        || !fence(state, [pos.x, pos.y, pos.z])
    {
        return false;
    }
    let Some(slot) = state.entities.slot_of(actor) else {
        return false;
    };
    let uuid = state.entities.row(slot).uuid;
    (0..state.entities.len()).any(|index| {
        let slot = EntitySlot(index as u32);
        if state.entities.row(slot).entity_life.leash != Some(Holder::Entity(uuid)) {
            return false;
        }
        let at = state.entities.pos(slot);
        (at[0] - f64::from(pos.x)).powi(2)
            + (at[1] - f64::from(pos.y)).powi(2)
            + (at[2] - f64::from(pos.z)).powi(2)
            <= 144.0
    })
}

pub(crate) fn use_fence(state: &mut TickState, actor: EntityId, pos: BlockPos) -> bool {
    if state
        .players
        .get(&actor)
        .is_none_or(|p| p.spectator() || p.vitals.is_dead())
    {
        return false;
    }
    let p = [pos.x, pos.y, pos.z];
    if !fence(state, p) {
        return false;
    }
    let Some(slot) = state.entities.slot_of(actor) else {
        return false;
    };
    let uuid = state.entities.row(slot).uuid;
    let mut changed = false;
    for i in 0..state.entities.len() {
        let slot = EntitySlot(i as u32);
        if state.entities.row(slot).entity_life.leash == Some(Holder::Entity(uuid)) {
            let at = state.entities.pos(slot);
            let distance = (at[0] - f64::from(pos.x)).powi(2)
                + (at[1] - f64::from(pos.y)).powi(2)
                + (at[2] - f64::from(pos.z)).powi(2);
            if distance <= 144.0 {
                state.entities.set_leash(slot, Some(Holder::Fence(p)));
                changed = true;
            }
        }
    }
    changed
}
pub(crate) fn knot_interact(state: &mut TickState, slot: EntitySlot) -> bool {
    if crate::mounts::name(state, slot) != "minecraft:leash_knot" {
        return false;
    }
    let at = state.entities.pos(slot);
    let p = [
        at[0].floor() as i32,
        at[1].floor() as i32,
        at[2].floor() as i32,
    ];
    for i in 0..state.entities.len() {
        let s = EntitySlot(i as u32);
        if state.entities.row(s).entity_life.leash == Some(Holder::Fence(p)) {
            detach(state, s);
        }
    }
    true
}
pub(crate) fn tick(state: &mut TickState, changes: &mut ChangeSet) {
    let mut knots = std::mem::take(&mut state.leash_knots);
    knots.clear();
    for slot in state.entities.mob_slots_matching(|type_id| {
        mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
            .is_some_and(|contract| contract.name == "minecraft:leash_knot")
    }) {
        if crate::mounts::name(state, slot) == "minecraft:leash_knot" {
            knots.insert(
                state.entities.pos(slot).map(|x| x.floor() as i32),
                (state.entities.body(slot).id, false),
            );
        }
    }
    let leashed: Vec<_> = state.entities.leashed_slots().collect();
    for slot in leashed {
        let Some(holder) = state.entities.row(slot).entity_life.leash.clone() else {
            continue;
        };
        let anchor = match holder {
            Holder::Entity(uuid) => state.entities.slot_of_uuid(uuid).filter(|s| {
                let body = state.entities.body(*s);
                body.alive
                    && state
                        .players
                        .get(&body.id)
                        .is_none_or(|p| !p.spectator() && !p.vitals.is_dead())
            }),
            Holder::Fence(pos) => {
                if !fence(state, pos) {
                    None
                } else {
                    let knot = knots
                        .get(&pos)
                        .and_then(|(id, _)| state.entities.slot_of(*id));
                    knot.or_else(|| {
                        let id = state.registry.entity_types.id_of(
                            &Identifier::parse("minecraft:leash_knot").expect("static id"),
                        )?;
                        let knot = state.spawn_mob(
                            id,
                            [
                                f64::from(pos[0]) + 0.5,
                                f64::from(pos[1]) + 0.375,
                                f64::from(pos[2]) + 0.5,
                            ],
                            1.0,
                        );
                        changes.entities_spawned.push(knot);
                        knots.insert(pos, (knot, false));
                        state.entities.slot_of(knot)
                    })
                }
            }
        };
        let Some(anchor) = anchor else {
            detach(state, slot);
            continue;
        };
        let at = state.entities.pos(slot);
        let to = state.entities.pos(anchor);
        let distance = at.iter().zip(to).map(|(a, b)| (a - b).powi(2)).sum::<f64>();
        if !state.entities.body(slot).alive || distance > 144.0 {
            detach(state, slot);
            continue;
        }
        if let Holder::Fence(pos) = holder
            && let Some((_, used)) = knots.get_mut(&pos)
        {
            *used = true;
        }
        let holder_id = state.entities.body(anchor).id;
        let (body, row) = state.entities.parts_mut(slot);
        if row.appearance.leash_holder != Some(holder_id) {
            std::sync::Arc::make_mut(&mut row.appearance).leash_holder = Some(holder_id);
            state.appearance_changed.push(body.id);
        }
        if distance > 4.0 && !row.entity_life.sitting && row.entity_life.mount.rider.is_none() {
            let speed = f64::from(row.speed);
            let norm = mth::length2(to[0] - at[0], to[2] - at[2]).max(0.001);
            state.entities.set_vel(
                slot,
                [
                    (to[0] - at[0]) / norm * speed,
                    state.entities.vel(slot)[1],
                    (to[2] - at[2]) / norm * speed,
                ],
            );
        } else {
            state
                .entities
                .set_vel(slot, [0.0, state.entities.vel(slot)[1], 0.0]);
        }
    }
    for &(id, used) in knots.values() {
        if used {
            continue;
        }
        if let Some(slot) = state.entities.slot_of(id) {
            state.remove_entity(slot);
            changes.entities_removed.push(id);
        }
    }
    state.leash_knots = knots;
}
#[cfg(test)]
#[path = "leash_tests.rs"]
mod tests;
