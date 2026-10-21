//! Native military policy. Membership lives on resident entities, equipment in
//! canonical entity slots; no second roster or virtual stockpile is maintained.
use super::{SettlementId, buildings, inventory};
use crate::entities::EntitySlot;
use crate::tick::TickState;
use crate::villager::VillagerProfession;
use buildings::{BuildingId, BuildingKind};
use mc_data::ItemStack;
use uuid::Uuid;

/// Death-drop chance above 1.0 is a guaranteed drop (see copper golem/piglin custody).
const CUSTODY_DROP_CHANCE: f32 = 2.0;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Order {
    Follow(Uuid),
    Guard([i32; 3]),
    Patrol,
    Return,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Soldier {
    pub barracks: BuildingId,
    pub order: Order,
    pub next_attack: u64,
}

fn member(state: &TickState, slot: EntitySlot, id: SettlementId) -> bool {
    state.entities.body(slot).alive
        && state.entities.row(slot).villager.as_ref().is_some_and(|v| {
            v.settlement == Some(id) && v.soldier.is_some() && v.conversion_time.is_none()
        })
}
fn storage(state: &TickState, id: SettlementId) -> Vec<inventory::Image> {
    let mut images = Vec::new();
    let mut physical = std::collections::BTreeSet::new();
    for position in buildings::storage_points(state, id) {
        let endpoint = inventory::Endpoint::Container {
            dimension: state.dimension.name().into(),
            position,
        };
        let Ok(cells) = crate::plugin_inventories::container_cells(
            state,
            &mc_script::ScriptBlockPosition::new(position[0], position[1], position[2]),
        ) else {
            continue;
        };
        if cells
            .iter()
            .any(|cell| physical.contains(&[cell.x, cell.y, cell.z]))
        {
            continue;
        }
        if let Ok(image) = inventory::load(state, &endpoint) {
            physical.extend(cells.iter().map(|cell| [cell.x, cell.y, cell.z]));
            images.push(image);
        }
    }
    images
}

fn swing() -> crate::combat::Swing {
    crate::combat::Swing {
        ticks_since_reset: 100,
        falling: false,
        on_ground: true,
        sprinting: false,
        crit_blocked: false,
        melee_bonus: 0.0,
        attack_speed_multiplier: 1.0,
    }
}

fn equipment_rank(state: &TickState, stack: &ItemStack, index: usize) -> Option<f32> {
    if stack.is_empty() {
        return None;
    }
    let rules = state.registry.item_rules();
    if index == 0 {
        let name = state.registry.items.name_of(stack.item_id)?.path();
        return (name.ends_with("_sword") || name.ends_with("_axe"))
            .then(|| crate::combat::melee_hit(&rules, stack, swing()).damage);
    }
    let facts = state
        .registry
        .item_facts
        .facts_for_stack(stack, &state.registry.items)?;
    let armor = facts.armor.as_ref()?;
    (facts.equippable_slot.as_deref() == Some(["head", "chest", "legs", "feet"][index - 2]))
        .then_some(armor.armor + armor.toughness * 0.1)
}

fn vacancy(state: &TickState, id: SettlementId) -> Result<BuildingId, String> {
    let record = state.settlements.get(id).ok_or("Settlement not found")?;
    if !record
        .buildings
        .values()
        .any(|b| b.kind == BuildingKind::Barracks)
    {
        return Err("No free barracks bed".into());
    }
    let mut occupied = std::collections::BTreeMap::<BuildingId, usize>::new();
    for slot in state
        .entities
        .slots()
        .filter(|&slot| member(state, slot, id))
    {
        let barracks = state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .soldier
            .as_ref()
            .unwrap()
            .barracks;
        *occupied.entry(barracks).or_default() += 1;
    }
    record
        .buildings
        .values()
        .filter(|b| b.kind == BuildingKind::Barracks)
        .find(|b| occupied.get(&b.id).copied().unwrap_or(0) < buildings::job_slots(b.kind, b.level))
        .map(|b| b.id)
        .ok_or_else(|| "No free barracks bed".into())
}

/// Select an idle adult and a free certified barracks bed. Equipment and stock
/// are committed together before changing profession, so a failed debit cannot
/// leave a half-equipped recruit or duplicate a weapon on retry.
pub(crate) fn recruit(state: &mut TickState, id: SettlementId) -> Result<Uuid, String> {
    let barracks = vacancy(state, id)?;
    let slot = state
        .entities
        .slots()
        .find(|&s| {
            let row = state.entities.row(s);
            state.entities.body(s).alive
                && row.villager.as_ref().is_some_and(|v| {
                    v.settlement == Some(id)
                        && v.age >= 0
                        && !v.wandering
                        && v.conversion_time.is_none()
                        && v.data.profession == VillagerProfession::None
                        && v.job_site.is_none()
                        && v.soldier.is_none()
                })
        })
        .ok_or("No idle adult resident")?;
    let uuid = state.entities.row(slot).uuid;
    let before = inventory::load(state, &inventory::Endpoint::VillagerEquipment { uuid })
        .map_err(|e| format!("Equipment unavailable: {e:?}"))?;
    let mut equipped = before.slots.clone();
    let images = storage(state, id);
    let mut stocks: Vec<_> = images.iter().map(|i| i.slots.clone()).collect();
    for index in [0, 2, 3, 4, 5] {
        if !equipped[index].is_empty() {
            continue;
        }
        let read_state: &TickState = state;
        let best = stocks
            .iter()
            .enumerate()
            .flat_map(|(container, slots)| {
                slots.iter().enumerate().filter_map(move |(cell, stack)| {
                    equipment_rank(read_state, stack, index).map(|rank| (container, cell, rank))
                })
            })
            .max_by(|a, b| a.2.total_cmp(&b.2));
        if let Some((container, cell, _)) = best {
            equipped[index] = stocks[container][cell].clone();
            equipped[index].count = 1;
            stocks[container][cell].count -= 1;
            if stocks[container][cell].is_empty() {
                stocks[container][cell] = ItemStack::EMPTY;
            }
        }
    }
    let mut changes: Vec<_> = images
        .into_iter()
        .zip(stocks)
        .filter(|(before, after)| before.slots != *after)
        .map(|(before, after)| inventory::Change { before, after })
        .collect();
    if equipped != before.slots {
        changes.push(inventory::Change {
            before,
            after: equipped,
        });
    }
    if !changes.is_empty() {
        inventory::commit(state, changes).map_err(|e| format!("Cannot equip recruit: {e:?}"))?;
    }
    // Issued gear is settlement custody: guarantee its drop on death (2.0, as
    // copper golem/piglin custody) instead of the 8.5% default.
    let row = state.entities.row_mut(slot);
    for (chance, stack) in row
        .loot
        .drop_chances
        .iter_mut()
        .zip(&row.appearance.equipment)
    {
        if stack.is_some() {
            *chance = CUSTODY_DROP_CHANCE;
        }
    }
    let v = state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .expect("validated resident");
    v.data.profession = VillagerProfession::Soldier;
    v.soldier = Some(Soldier {
        barracks,
        order: Order::Patrol,
        next_attack: 0,
    });
    v.sleeping = false;
    v.navigation_path.clear();
    v.navigation_target = None;
    v.navigation_repath_at = 0;
    crate::villager::sync_appearance(state, slot);
    state
        .villager_pois
        .dirty_claim(state.entities.body(slot).id);
    Ok(uuid)
}

/// Full-storage refusal is atomic: the resident keeps both role and equipment.
pub(crate) fn dismiss(state: &mut TickState, id: SettlementId, uuid: Uuid) -> Result<(), String> {
    let slot = state
        .entities
        .slot_of_uuid(uuid)
        .filter(|&s| member(state, s, id))
        .ok_or("Soldier not found")?;
    let before = inventory::load(state, &inventory::Endpoint::VillagerEquipment { uuid })
        .map_err(|e| format!("Equipment unavailable: {e:?}"))?;
    let mut equipment = before.slots.clone();
    let images = storage(state, id);
    let mut stocks: Vec<_> = images.iter().map(|i| i.slots.clone()).collect();
    for moving in &mut equipment {
        if moving.is_empty() {
            continue;
        }
        let maximum = state.registry.item_rules().max_stack(moving);
        for empty in [false, true] {
            for slots in &mut stocks {
                for stored in slots {
                    if moving.is_empty()
                        || stored.is_empty() != empty
                        || (!stored.is_empty()
                            && !mc_data::inventory_semantics_26_1_2::can_stack(stored, moving))
                    {
                        continue;
                    }
                    let count = moving
                        .count
                        .min((maximum - if stored.is_empty() { 0 } else { stored.count }).max(0));
                    if count == 0 {
                        continue;
                    }
                    if stored.is_empty() {
                        *stored = moving.clone();
                        stored.count = 0;
                    }
                    stored.count += count;
                    moving.count -= count;
                }
            }
        }
        if !moving.is_empty() {
            return Err("Settlement storage has no room for returned equipment".into());
        }
        *moving = ItemStack::EMPTY;
    }
    let mut changes: Vec<_> = images
        .into_iter()
        .zip(stocks)
        .filter(|(before, after)| before.slots != *after)
        .map(|(before, after)| inventory::Change { before, after })
        .collect();
    if before.slots != equipment {
        changes.push(inventory::Change {
            before,
            after: equipment,
        });
    }
    if !changes.is_empty() {
        inventory::commit(state, changes).map_err(|e| format!("Cannot return equipment: {e:?}"))?;
    }
    state.entities.row_mut(slot).loot.drop_chances =
        crate::entity_loot::EntityLootState::default().drop_chances;
    let v = state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .expect("validated soldier");
    v.soldier = None;
    v.data.profession = VillagerProfession::None;
    v.navigation_path.clear();
    v.navigation_target = None;
    v.navigation_repath_at = 0;
    crate::villager::sync_appearance(state, slot);
    state
        .villager_pois
        .dirty_claim(state.entities.body(slot).id);
    Ok(())
}

pub(crate) fn order(
    state: &mut TickState,
    id: SettlementId,
    target: Option<Uuid>,
    order: Order,
) -> Result<usize, String> {
    if let Order::Guard(pos) = &order {
        let record = state.settlements.get(id).ok_or("Settlement not found")?;
        let distance: i128 = pos
            .iter()
            .zip(record.anchor)
            .map(|(a, b)| (i128::from(*a) - i128::from(b)).pow(2))
            .sum();
        if distance > i128::from(record.radius).pow(2) {
            return Err("Guard post must be inside the settlement".into());
        }
    }
    let slots: Vec<_> = state
        .entities
        .slots()
        .filter(|&s| {
            member(state, s, id) && target.is_none_or(|uuid| state.entities.row(s).uuid == uuid)
        })
        .collect();
    if slots.is_empty() {
        return Err("No matching soldiers".into());
    }
    for &slot in &slots {
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.soldier.as_mut().unwrap().order = order.clone();
        v.navigation_path.clear();
        v.navigation_target = None;
        v.navigation_repath_at = 0;
        state
            .villager_pois
            .dirty_claim(state.entities.body(slot).id);
    }
    Ok(slots.len())
}

/// One soldier as the roster shows it.
pub(crate) struct Row {
    pub uuid: Uuid,
    pub health: f32,
    pub order: Order,
    pub equipment: Vec<String>,
}

pub(crate) fn rows(state: &TickState, id: SettlementId) -> Vec<Row> {
    state
        .entities
        .slots()
        .filter(|&s| member(state, s, id))
        .map(|s| {
            let row = state.entities.row(s);
            Row {
                uuid: row.uuid,
                health: row.health,
                order: row
                    .villager
                    .as_ref()
                    .unwrap()
                    .soldier
                    .as_ref()
                    .unwrap()
                    .order
                    .clone(),
                equipment: row
                    .appearance
                    .equipment
                    .iter()
                    .flatten()
                    .filter_map(|stack| {
                        state
                            .registry
                            .items
                            .name_of(stack.item_id)
                            .map(ToString::to_string)
                    })
                    .collect(),
            }
        })
        .collect()
}

pub(crate) fn list(state: &TickState, id: SettlementId) -> Vec<String> {
    rows(state, id)
        .into_iter()
        .map(|row| {
            format!(
                "{} — {:.0} HP — {:?} — {}",
                row.uuid,
                row.health,
                row.order,
                row.equipment.join(", ")
            )
        })
        .collect()
}

/// At most one recruitment attempt per hundred ticks across unowned settlements.
/// Owned armies are staffed explicitly; unloaded settlements never catch up.
pub(crate) fn tick(state: &mut TickState) {
    if !state.tick.is_multiple_of(100) {
        return;
    }
    let count = state
        .settlements
        .records()
        .filter(|record| record.owner.is_none())
        .count();
    if count == 0 {
        return;
    }
    let selected = state
        .settlements
        .records()
        .filter(|record| record.owner.is_none())
        .nth((state.tick / 100) as usize % count)
        .map(|r| (r.id, r.anchor));
    if let Some((id, anchor)) = selected
        && state
            .world
            .is_active(anchor[0].div_euclid(16), anchor[2].div_euclid(16))
    {
        let _ = recruit(state, id);
    }
}

#[cfg(test)]
#[path = "army_tests.rs"]
pub(crate) mod tests;
