//! Precomputed per-protocol entity-type flags for the plan hot path.
//!
//! The table is built once from the exact 26.1.2 contracts (names are
//! matched only here, during init). Per-candidate lookups are a plain index
//! read: no name/string comparisons, and out-of-range ids read as false.
use mc_data::entity_types::{ENTITY_TYPE_COUNT, entity_type_contract_26_1_2_by_protocol_id};
use std::sync::LazyLock;

const BOSS: u8 = 0x01;
const LEASH: u8 = 0x02;

static FLAGS: LazyLock<[u8; ENTITY_TYPE_COUNT]> = LazyLock::new(|| {
    let mut table = [0u8; ENTITY_TYPE_COUNT];
    for (id, slot) in table.iter_mut().enumerate() {
        let Some(contract) = entity_type_contract_26_1_2_by_protocol_id(id as u32) else {
            continue;
        };
        match contract.name {
            "minecraft:wither" | "minecraft:ender_dragon" => *slot = BOSS,
            "minecraft:leash_knot" => *slot = LEASH,
            _ => {}
        }
    }
    table
});

pub(super) fn is_boss(type_id: u32) -> bool {
    usize::try_from(type_id)
        .ok()
        .and_then(|id| FLAGS.get(id))
        .is_some_and(|flags| flags & BOSS != 0)
}

pub(super) fn is_leash_knot(type_id: u32) -> bool {
    usize::try_from(type_id)
        .ok()
        .and_then(|id| FLAGS.get(id))
        .is_some_and(|flags| flags & LEASH != 0)
}
