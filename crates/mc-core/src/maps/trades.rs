//! Cartographer offers resolve through the same bounded locate worker as loot.
use crate::boundary::{ConnectionId, EntityId, Hand, PlayerInput};
use crate::tick::TickState;
use crate::villager::{VillagerData, VillagerProfession, VillagerTradeOffer};
use mc_data::{Identifier, ItemStack};

#[derive(Debug, Clone, Copy)]
struct Spec {
    level: u8,
    tag: &'static str,
    icon: &'static str,
    price: i32,
    xp: i32,
}
#[derive(Debug)]
pub(crate) struct Pending {
    uuid: uuid::Uuid,
    data: VillagerData,
    specs: Vec<Spec>,
    openers: Vec<(ConnectionId, Hand)>,
}
fn specs(data: VillagerData, seed: u128) -> Vec<Spec> {
    // VillagerTrades.registerBasicExplorerMapTrades / bootstrap, local 26.1.2 javap.
    const REGIONAL: &[(&str, &str, &[&str])] = &[
        (
            "on_taiga_village_maps",
            "village_taiga",
            &["swamp", "snow", "plains"],
        ),
        (
            "on_swamp_explorer_maps",
            "swamp_hut",
            &["taiga", "snow", "jungle"],
        ),
        (
            "on_snowy_village_maps",
            "village_snowy",
            &["taiga", "swamp"],
        ),
        (
            "on_savanna_village_maps",
            "village_savanna",
            &["plains", "jungle", "desert"],
        ),
        (
            "on_plains_village_maps",
            "village_plains",
            &["taiga", "snow", "savanna", "desert"],
        ),
        (
            "on_jungle_explorer_maps",
            "jungle_temple",
            &["swamp", "savanna", "desert"],
        ),
        (
            "on_desert_village_maps",
            "village_desert",
            &["savanna", "jungle"],
        ),
    ];
    let mut out = Vec::new();
    if data.level >= 2 {
        let eligible: Vec<_> = REGIONAL
            .iter()
            .filter(|(_, _, types)| types.contains(&data.kind.name()))
            .collect();
        if !eligible.is_empty() {
            let &(tag, icon, _) = eligible[(seed % eligible.len() as u128) as usize];
            out.push(Spec {
                level: 2,
                tag,
                icon,
                price: 8,
                xp: 5,
            });
        }
    }
    if data.level >= 3 {
        out.push(if seed & 1 == 0 {
            Spec {
                level: 3,
                tag: "on_ocean_explorer_maps",
                icon: "monument",
                price: 13,
                xp: 10,
            }
        } else {
            Spec {
                level: 3,
                tag: "on_trial_chambers_maps",
                icon: "trial_chambers",
                price: 12,
                xp: 10,
            }
        });
    }
    if data.level >= 5 {
        out.push(Spec {
            level: 5,
            tag: "on_woodland_explorer_maps",
            icon: "mansion",
            price: 14,
            xp: 30,
        });
    }
    out
}
fn has_offer(state: &TickState, merchant: EntityId, spec: Spec) -> bool {
    state
        .entities
        .slot_of(merchant)
        .and_then(|slot| state.entities.row(slot).villager.as_ref())
        .is_some_and(|v| {
            v.offers.iter().any(|offer| {
                super::map_id(&offer.result)
                    .and_then(|id| state.maps.records.get(&id))
                    .is_some_and(|map| {
                        map.decorations.values().any(|d| {
                            d.kind.strip_prefix("minecraft:").unwrap_or(&d.kind) == spec.icon
                        })
                    })
            })
        })
}
pub(crate) fn before_open(
    state: &mut TickState,
    merchant: EntityId,
    connection: ConnectionId,
    hand: Hand,
) -> bool {
    if let Some(pending) = state.maps.pending_trades.get_mut(&merchant) {
        if !pending.openers.iter().any(|(conn, _)| *conn == connection) {
            pending.openers.push((connection, hand));
        }
        return true;
    }
    let Some(slot) = state.entities.slot_of(merchant) else {
        return false;
    };
    let row = state.entities.row(slot);
    let Some(villager) = row.villager.as_ref() else {
        return false;
    };
    if villager.data.profession != VillagerProfession::Cartographer {
        return false;
    }
    let uuid = row.uuid;
    let data = villager.data;
    let specs: Vec<_> = specs(data, uuid.as_u128())
        .into_iter()
        .filter(|spec| {
            !has_offer(state, merchant, *spec)
                && !state.maps.attempted_trades.contains(&(uuid, spec.level))
        })
        .collect();
    if specs.is_empty() {
        return false;
    }
    state.maps.pending_trades.insert(
        merchant,
        Pending {
            uuid,
            data,
            specs,
            openers: vec![(connection, hand)],
        },
    );
    if !submit(state, merchant) {
        state.maps.pending_trades.remove(&merchant);
    }
    true
}
fn submit(state: &mut TickState, merchant: EntityId) -> bool {
    let pending = &state.maps.pending_trades[&merchant];
    let spec = pending.specs[0];
    let Some(slot) = state.entities.slot_of(merchant) else {
        return false;
    };
    let pos = state.entities.pos(slot).map(|n| n.floor() as i32);
    let referenced = state
        .maps
        .records
        .values()
        .filter(|m| m.dimension == state.dimension)
        .filter_map(|m| {
            m.destination
                .map(|[x, y, z]| mc_world::BlockPos { x, y, z })
        })
        .collect();
    crate::command_locate::submit_structure_lookup(
        state,
        crate::command_locate::StructureLookupRequest {
            connection: pending.openers[0].0,
            origin: (pos[0], pos[1], pos[2]),
            destination: Identifier::parse(format!("minecraft:{}", spec.tag))
                .expect("vanilla destination"),
            search_radius: 100,
            skip_existing_chunks: true,
            referenced,
            reply: Box::new(move |state, hit| complete(state, merchant, hit)),
        },
    )
}
fn complete(state: &mut TickState, merchant: EntityId, hit: Option<mc_world::BlockPos>) {
    let Some(mut pending) = state.maps.pending_trades.remove(&merchant) else {
        return;
    };
    let Some(slot) = state.entities.slot_of(merchant) else {
        return;
    };
    let row = state.entities.row(slot);
    if !state.entities.body(slot).alive
        || row.uuid != pending.uuid
        || row.villager.as_ref().is_none_or(|v| v.data != pending.data)
    {
        return;
    }
    if hit.is_some_and(|pos| {
        state.maps.records.values().any(|map| {
            map.dimension == state.dimension && map.destination == Some([pos.x, pos.y, pos.z])
        })
    }) {
        state.maps.pending_trades.insert(merchant, pending);
        if !submit(state, merchant) {
            state.maps.pending_trades.remove(&merchant);
        }
        return;
    }
    let spec = pending.specs.remove(0);
    if let Some(pos) = hit
        && !has_offer(state, merchant, spec)
    {
        let item = |name| {
            state
                .registry
                .items
                .id_of(&Identifier::parse(name).expect("vanilla item"))
        };
        if let (Some(emerald), Some(compass)) =
            (item("minecraft:emerald"), item("minecraft:compass"))
            && let Ok(result) = super::create_exploration_map(
                state,
                [pos.x, pos.y, pos.z],
                state.dimension,
                &format!("minecraft:{}", spec.icon),
                2,
            )
        {
            state
                .entities
                .row_mut(slot)
                .villager
                .as_mut()
                .expect("checked merchant")
                .offers
                .push(VillagerTradeOffer {
                    cost_a: ItemStack::new(emerald, spec.price),
                    cost_b: Some(ItemStack::new(compass, 1)),
                    result,
                    uses: 0,
                    max_uses: 12,
                    reward_exp: true,
                    xp: spec.xp,
                    special_price: 0,
                    demand: 0,
                    price_multiplier: 0.2,
                });
        }
    }
    state
        .maps
        .attempted_trades
        .insert((pending.uuid, spec.level));
    if pending.specs.is_empty() {
        for (connection, hand) in pending.openers {
            if state.connections.entity_of(connection).is_some() {
                state.deferred_uses.push(PlayerInput::InteractEntity {
                    connection,
                    target: merchant,
                    hand,
                    location: [0.0; 3],
                });
            }
        }
    } else {
        state.maps.pending_trades.insert(merchant, pending);
        if !submit(state, merchant) {
            state.maps.pending_trades.remove(&merchant);
        }
    }
}
