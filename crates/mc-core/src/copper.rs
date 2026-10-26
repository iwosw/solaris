//! Data-driven copper weathering for all 14 full 8-variant families
//! (WeatheringCopper NEXT/PREVIOUS BiMaps). State changes retain each
//! block's inventory and all properties; paired copper chests always
//! share one weathering image.
use crate::boundary::{BlockPos, EntityId, Hand};
use crate::tick::TickState;
use mc_data::Identifier;
use mc_world::{BlockRegistry, BlockStateId};

pub(crate) const AGES: [&str; 4] = ["unaffected", "exposed", "weathered", "oxidized"];
pub(crate) const POSES: [&str; 4] = ["standing", "sitting", "running", "star"];

/// Every weathering family: the bare (age-0, unwaxed) block path, derived
/// from the vanilla registry (each has all 8 wax/age variants).
/// `copper_block` (WeatheringCopperFullBlock age-0 alias of exposed_copper)
/// participates in weathering at age 0 only; aged cut-family blocks use
/// their own `cut_copper`-rooted names.
const FAMILIES: [&str; 15] = [
    "chiseled_copper",
    "copper_bars",
    "copper_block",
    "copper_bulb",
    "copper_chain",
    "copper_chest",
    "copper_door",
    "copper_golem_statue",
    "copper_grate",
    "copper_lantern",
    "copper_trapdoor",
    "cut_copper",
    "cut_copper_slab",
    "cut_copper_stairs",
    "lightning_rod",
];

pub(crate) fn chest(path: &str) -> bool {
    family(path).is_some_and(|(name, _, _)| name == "copper_chest")
}

pub(crate) fn family(path: &str) -> Option<(&str, u8, bool)> {
    let waxed = path.starts_with("waxed_");
    let path = path.strip_prefix("waxed_").unwrap_or(path);
    let (path, age) = if let Some(path) = path.strip_prefix("exposed_") {
        (path, 1)
    } else if let Some(path) = path.strip_prefix("weathered_") {
        (path, 2)
    } else if let Some(path) = path.strip_prefix("oxidized_") {
        (path, 3)
    } else {
        (path, 0)
    };
    // copper_block is the age-0 full-block alias; it never takes aged forms.
    if path == "copper_block" && (waxed || age > 0) {
        return None;
    }
    FAMILIES.contains(&path).then_some((path, age, waxed))
}

pub(crate) fn name(family: &str, age: u8, waxed: bool) -> String {
    format!(
        "minecraft:{}{}{family}",
        if waxed { "waxed_" } else { "" },
        match age {
            1 => "exposed_",
            2 => "weathered_",
            3 => "oxidized_",
            _ => "",
        }
    )
}

pub(crate) fn transform(blocks: &BlockRegistry, id: u32, age: u8, waxed: bool) -> Option<u32> {
    let state = blocks.by_id(BlockStateId(id))?;
    let (family, _, _) = family(state.block.id.path())?;
    let name = Identifier::parse(name(family, age, waxed)).ok()?;
    blocks
        .by_name_and_props(&name, &state.properties)
        .map(|id| id.0)
}

pub(crate) fn same_family(a: &Identifier, b: &Identifier) -> bool {
    family(a.path())
        .zip(family(b.path()))
        .is_some_and(|(a, b)| a.0 == b.0)
}

pub(crate) fn compatible_chests(a: &Identifier, b: &Identifier) -> bool {
    a == b || (chest(a.path()) && chest(b.path()))
}

/// Placement unifies both halves to the least oxidized variant; mixed wax
/// status removes wax, as CopperChestBlock.getLeastOxidizedChestOfConnectedBlocks.
pub(crate) fn joined(blocks: &BlockRegistry, a: u32, b: u32) -> Option<(u32, u32)> {
    let a_part = family(blocks.by_id(BlockStateId(a))?.block.id.path())?;
    let b_part = family(blocks.by_id(BlockStateId(b))?.block.id.path())?;
    if a_part.0 != "copper_chest" || b_part.0 != a_part.0 {
        return None;
    }
    let age = a_part.1.min(b_part.1);
    let waxed = a_part.2 && b_part.2;
    Some((
        transform(blocks, a, age, waxed)?,
        transform(blocks, b, age, waxed)?,
    ))
}

/// Called only for a cell selected by the existing native random-tick sampler.
/// Takes two independent draws, matching ChangeOverTimeBlock.changeOverTime
/// (nextFloat gates 0.05688889) + getNextState (second nextFloat gates the
/// scan chance): sharing one roll corrupts the scan cadence ~17x on
/// clustered same-age builds.
pub(crate) fn random_tick(
    state: &TickState,
    pos: BlockPos,
    id: u32,
    gate_roll: u32,
    scan_roll: u32,
) -> Option<Vec<(BlockPos, u32)>> {
    let block = state.registry.blocks.by_id(BlockStateId(id))?;
    let (_, age, waxed) = family(block.block.id.path())?;
    // Pinned ChangeOverTimeBlock.changeOverTime: nextFloat < 0.05688889f
    // gates the Manhattan-4 same-class scan; getNextState draws a SECOND
    // nextFloat for the scan chance ((older+1)/(older+same+1))^2.
    // nextFloat is 24 random bits over 2^24: 0.05688889f * 2^24 =
    // 954437.1875 -> top-24-bits < 954438 (bits <= 954437). With no older
    // chance is 1, so the gate alone decides.
    if waxed || age == 3 || gate_roll % (1 << 24) >= 954438 {
        return None;
    }
    if !oxidation_scan_passes(state, pos, scan_roll) {
        return None;
    }
    let next = transform(&state.registry.blocks, id, age + 1, false)?;
    let mut writes = vec![(pos, next)];
    if chest(block.block.id.path()) {
        for other in crate::chests::storage_cells(&state.registry.blocks, pos, &|p| {
            state.world.block_at(p.x, p.y, p.z)
        }) {
            if other == pos {
                continue;
            }
            let old = state.world.block_at(other.x, other.y, other.z)?;
            writes.push((
                other,
                transform(&state.registry.blocks, old, age + 1, false)?,
            ));
        }
    }
    Some(writes)
}

/// Manhattan-4 same-class oxidation scan: counts older (higher-age) and same-age
/// weathering neighbours; passes with ((older+1)/(older+same+1))^2.
/// Waxed neighbours are skipped: vanilla getNextState only counts
/// instanceof ChangeOverTimeBlock, and waxed blocks are plain Blocks.
fn oxidation_scan_passes(state: &TickState, pos: BlockPos, roll: u32) -> bool {
    let Some(here) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return false;
    };
    let Some((_, age, _)) = family_of_state(state, here) else {
        return false;
    };
    let mut older = 0u32;
    let mut same = 0u32;
    for dx in -4i32..=4 {
        for dy in -4i32..=4 {
            for dz in -4i32..=4 {
                if dx.abs() + dy.abs() + dz.abs() > 4 || (dx == 0 && dy == 0 && dz == 0) {
                    continue;
                }
                let cell = BlockPos {
                    x: pos.x + dx,
                    y: pos.y + dy,
                    z: pos.z + dz,
                };
                let Some(id) = state.world.block_at(cell.x, cell.y, cell.z) else {
                    continue;
                };
                let Some((_, nage, nwaxed)) = family_of_state(state, id) else {
                    continue;
                };
                if nwaxed {
                    continue;
                }
                if nage > age {
                    older += 1;
                } else if nage == age {
                    same += 1;
                } else {
                    return false;
                }
            }
        }
    }
    // Exact float compare via integer cross-multiplication:
    // roll24/2^24 < ((older+1)/(older+same+1))^2.
    let num = u64::from(older + 1) * u64::from(older + 1) * (1 << 24);
    let den = u64::from(older + same + 1) * u64::from(older + same + 1);
    u64::from(roll % (1 << 24)) * den < num
}

pub(crate) fn use_on(state: &mut TickState, actor: EntityId, hand: Hand, pos: BlockPos) -> bool {
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    if player.spectator() {
        return false;
    }
    let index = if hand == Hand::Main {
        usize::from(player.inventory.selected)
    } else {
        crate::inventory::OFF_HAND
    };
    let held = &player.inventory.slots[index];
    let item = state
        .registry
        .items
        .name_of(held.item_id)
        .map(|id| id.path())
        .unwrap_or("");
    let Some(id) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return false;
    };
    let Some(block) = state.registry.blocks.by_id(BlockStateId(id)) else {
        return false;
    };
    let Some((family, age, waxed)) = family(block.block.id.path()) else {
        return false;
    };
    let honeycomb = item == "honeycomb";
    let axe = item.ends_with("_axe");
    let mut cells = Vec::new();
    if honeycomb && !waxed {
        if let Some(next) = transform(&state.registry.blocks, id, age, true) {
            cells.push((pos, next));
        }
    } else if axe && family == "copper_golem_statue" && age == 0 && !waxed {
        let before = held.clone();
        let Ok(wear) = prepare_tool_wear(state, actor, hand) else {
            return true;
        };
        if crate::copper_golem::revive(state, pos) {
            apply_tool_wear(state, actor, wear);
            crate::advancements::accepted_block(
                state,
                actor,
                "minecraft:item_used_on_block",
                pos,
                Some(id),
                Some(before),
            );
        }
        return true;
    } else if axe && (waxed || age > 0) {
        if let Some(next) = transform(
            &state.registry.blocks,
            id,
            if waxed { age } else { age - 1 },
            false,
        ) {
            cells.push((pos, next));
        }
    } else if family == "copper_golem_statue" && !axe && !honeycomb {
        let mut props = block.properties.clone();
        if let Some((_, value)) = props.iter_mut().find(|(key, _)| key == "copper_golem_pose") {
            let pose = POSES.iter().position(|p| p == value).unwrap_or(0);
            *value = POSES[(pose + 1) % 4].into();
        }
        if let Some(next) = state
            .registry
            .blocks
            .by_name_and_props(&block.block.id, &props)
        {
            cells.push((pos, next.0));
        }
    } else {
        return false;
    }
    if family == "copper_chest"
        && let Some((_, next)) = cells.first().copied()
    {
        let (_, new_age, new_wax) = family_of_state(state, next).expect("copper image");
        for other in crate::chests::storage_cells(&state.registry.blocks, pos, &|p| {
            state.world.block_at(p.x, p.y, p.z)
        }) {
            if other != pos
                && let Some(old) = state.world.block_at(other.x, other.y, other.z)
                && let Some(next) = transform(&state.registry.blocks, old, new_age, new_wax)
            {
                cells.push((other, next));
            }
        }
    }
    if cells.is_empty() {
        return false;
    }
    if honeycomb && crate::tick::held_debit_below_floor(state, actor, hand, 1) {
        return true;
    }
    let wear = if axe {
        let Ok(wear) = prepare_tool_wear(state, actor, hand) else {
            return true;
        };
        wear
    } else {
        None
    };
    let before = (honeycomb || axe).then(|| state.players[&actor].inventory.slots[index].clone());
    for (pos, next) in cells {
        state.world.set_block(pos.x, pos.y, pos.z, next);
        state.merge_block_writes.push((pos, next));
    }
    if honeycomb {
        let player = state.players.get_mut(&actor).expect("player");
        let connection = player.connection;
        let effects = player.consume_held(hand, 1);
        for change in effects.changes {
            state.push_player(connection, change);
        }
    } else if axe {
        apply_tool_wear(state, actor, wear);
    }
    if let Some(before) = before {
        crate::advancements::accepted_block(
            state,
            actor,
            "minecraft:item_used_on_block",
            pos,
            Some(id),
            Some(before),
        );
    }
    true
}

fn family_of_state(state: &TickState, id: u32) -> Option<(&str, u8, bool)> {
    family(
        state
            .registry
            .blocks
            .by_id(BlockStateId(id))?
            .block
            .id
            .path(),
    )
}

pub(crate) fn prepare_tool_wear(
    state: &TickState,
    actor: EntityId,
    hand: Hand,
) -> Result<Option<crate::player::NativeToolWearPlan>, ()> {
    let Some(player) = state.players.get(&actor) else {
        return Ok(None);
    };
    if player.creative() || player.spectator() {
        return Ok(None);
    }
    let plan = player.plan_tool_wear(&state.registry.item_rules(), hand, 1);
    if crate::plugin_inventories::below_player_debit_floors(state, actor, &[], plan.broken_slot()) {
        return Err(());
    }
    Ok(Some(plan))
}

pub(crate) fn apply_tool_wear(
    state: &mut TickState,
    actor: EntityId,
    plan: Option<crate::player::NativeToolWearPlan>,
) {
    let Some(plan) = plan else { return };
    let Some(player) = state.players.get_mut(&actor) else {
        return;
    };
    let connection = player.connection;
    let effects = player.apply_tool_wear(plan);
    for change in effects.changes {
        state.push_player(connection, change);
    }
}

#[cfg(test)]
#[path = "copper_tests.rs"]
mod tests;
