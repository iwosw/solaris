//! Dropped-item fire immunity from report-authored damage_resistant defaults.

use crate::survival::DamageKind;
use crate::tick::TickState;

pub(crate) fn resistant(
    state: &TickState,
    item: &crate::entities::StackRow,
    kind: DamageKind,
) -> bool {
    matches!(kind, DamageKind::Lava | DamageKind::InFire)
        && !item.stack.as_ref().is_some_and(|stack| {
            stack
                .extended()
                .preserved_components
                .iter()
                .any(|component| component.name == "!minecraft:damage_resistant")
        })
        && match &item.stack {
            Some(stack) => state
                .registry
                .item_facts
                .facts_for_stack(stack, &state.registry.items),
            None => state
                .registry
                .items
                .name_of(item.item_id)
                .and_then(|name| state.registry.item_facts.get(name)),
        }
        .is_some_and(|facts| facts.fire_resistant)
}
