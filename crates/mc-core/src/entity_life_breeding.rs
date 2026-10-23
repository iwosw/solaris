//! Pair compatibility is blueprint data; merge rechecks the same rule as AI.
use crate::boundary::EntityId;
use crate::entities::EntityKind;
use crate::registry::CoreRegistry;
use crate::tick::TickState;

pub(crate) fn offspring(registry: &CoreRegistry, first: u32, second: u32) -> Option<u32> {
    let a = registry.blueprint(first)?;
    let b = registry.blueprint(second)?;
    let child = if first == second {
        a.offspring?
    } else {
        let a = a.capabilities.crossbreed?;
        let b = b.capabilities.crossbreed?;
        if a.partner != second || b.partner != first || a.offspring != b.offspring {
            return None;
        }
        a.offspring
    };
    registry.blueprint(child).map(|_| child)
}

pub(crate) fn valid(state: &TickState, actor: EntityId, partner: EntityId, child: u32) -> bool {
    if actor == partner {
        return false;
    }
    let Some(a) = state.entities.slot_of(actor) else {
        return false;
    };
    let Some(b) = state.entities.slot_of(partner) else {
        return false;
    };
    for slot in [a, b] {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if !crate::entity_life::breed_ready(body, row)
            || !crate::entity_life::can_breed(state, body, row)
        {
            return false;
        }
    }
    let (EntityKind::Mob { type_id: first }, EntityKind::Mob { type_id: second }) =
        (state.entities.body(a).kind, state.entities.body(b).kind)
    else {
        return false;
    };
    offspring(&state.registry, first, second) == Some(child)
        && crate::target_grid::distance(state.entities.pos(a), state.entities.pos(b)) < 3.0
}
