//! Native death takes custody exactly once before the ordinary equipment roll.
use super::*;
use crate::plugin_inventories as inventories;

pub(super) fn record(state: &mut TickState, uuid: uuid::Uuid) {
    let Some((owner, handle)) =
        custodian(state, uuid).map(|(owner, handle)| (owner.to_owned(), handle.to_owned()))
    else {
        return;
    };
    let Ok(mut all) = records(state, &owner) else {
        crate::plugin::fail_native(state);
        return;
    };
    let Some(record) = all.get_mut(&handle) else {
        return;
    };
    if record.disposition == ScriptNpcLifecycle::Released || record.controls.death_cargo.is_some() {
        return;
    }
    let Some(slot) = state.entities.slots().find(|slot| {
        let body = state.entities.body(*slot);
        let row = state.entities.row(*slot);
        row.uuid == uuid && !body.alive
    }) else {
        return;
    };
    if !inventories::can_synchronize_entity_custody(state, slot) {
        crate::plugin::fail_native(state);
        return;
    }
    let source_revision = match inventories::native_npc_inventory_revision(state, &owner, &handle) {
        Ok(revision) => revision,
        Err(_) => {
            crate::plugin::fail_native(state);
            return;
        }
    };
    let revision = next_revision(state, &owner).max(source_revision);
    let pos = state.entities.pos(slot);
    let rotation = state.entities.rot(slot);
    let equipment = state.entities.row(slot).appearance.equipment.clone();
    let carry = state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .map(|v| v.inventory.clone())
        .unwrap_or_else(|| std::array::from_fn(|_| None));
    let mut loot = Vec::new();
    let drops = crate::command_rules::boolean(state, "mob_drops");
    // SDK armor slots are [hands,head,chest,legs,feet]; native columns are reversed.
    for (inventory, stacks) in [
        (ScriptNpcDeathInventory::Carry, carry.as_slice()),
        (ScriptNpcDeathInventory::Equipment, equipment.as_slice()),
    ] {
        for (native_slot, stack) in stacks.iter().enumerate() {
            let Some(stack) = stack.as_ref().filter(|stack| !stack.is_empty()) else {
                continue;
            };
            if !drops
                || stack.enchantments.iter().any(|enchantment| {
                    enchantment.level > 0 && enchantment.id.as_str() == "minecraft:vanishing_curse"
                })
            {
                continue;
            }
            let canonical_slot = if inventory == ScriptNpcDeathInventory::Equipment {
                [0, 1, 5, 4, 3, 2][native_slot]
            } else {
                native_slot
            };
            let mut remaining = stack.count;
            while remaining > 0 {
                let count = remaining.min(64);
                let mut exact = stack.clone();
                exact.count = count;
                remaining -= count;
                let image = inventories::to_script_item(state, &exact)
                    .expect("native item canonical death image");
                let entity = crate::tick::spawn_item_entity(state, pos, exact);
                let item_slot = state
                    .entities
                    .slot_of(entity)
                    .expect("native death loot body");
                loot.push(ScriptNpcDeathLoot::new(
                    inventory,
                    canonical_slot as u16,
                    state.entities.row(item_slot).uuid.to_string(),
                    image,
                ));
                state.plugin_spawned.push(entity);
            }
        }
    }
    let row = state.entities.row_mut(slot);
    if let Some(v) = &mut row.villager {
        v.inventory = std::array::from_fn(|_| None);
        v.navigation_path.clear();
        v.navigation_target = None;
        v.navigation_chunks.clear();
    }
    std::sync::Arc::make_mut(&mut row.appearance).equipment = std::array::from_fn(|_| None);
    state.appearance_changed.push(state.entities.body(slot).id);
    if inventories::synchronize_entity_custody(state, slot).is_err() {
        crate::plugin::fail_native(state);
        return;
    }
    let decision = state
        .plugins
        .get_domain(NATIVE, "npc/death-sequence")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
        + 1;
    let epoch = routes::death_epoch(state, &owner, &handle);
    let receipt = ScriptNpcDeathCargoReceipt::new(
        inventories::hash(&(uuid, decision)),
        handle.clone(),
        uuid.to_string(),
        record.dimension.clone(),
        ScriptPosition::try_new(pos[0], pos[1], pos[2]).expect("native death pose"),
        rotation[0],
        rotation[1],
        state.tick,
        source_revision,
        epoch,
        decision,
        loot,
    );
    record.disposition = ScriptNpcLifecycle::Dead;
    record.revision = revision;
    record.controls.revision = revision;
    record.controls.order = None;
    record.controls.order_revision = 0;
    record.controls.pending_work = None;
    record.controls.combat = None;
    record.controls.item_use = None;
    if let Some(work) = &mut record.controls.work {
        work.state = ScriptWorkState::Cancelled;
        work.reason = None;
        work.suspension_reason = None;
        work.due_tick = None;
        work.revision = revision;
    }
    record.controls.death_cargo = Some(Box::new(receipt.clone()));
    save_records(state, &owner, &all, revision);
    routes::record_death(state, &owner, &handle, &receipt, revision);
    state.plugins.set_domain(
        NATIVE,
        "npc/death-sequence",
        serde_json::Value::from(decision),
    );
}
