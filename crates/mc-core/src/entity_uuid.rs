//! Deterministic identity for server-spawned entities.
//!
//! A UUID is a pure function of `(world seed, spawn tick, wire id)`. Wire ids
//! are never handed out twice within one run and the tick clock is recovered
//! monotonically across restarts, so the triple is unique without any new
//! persisted counter, while two worlds fed the same inputs mint the same ids.
use crate::boundary::EntityId;
use crate::tick::TickState;

/// SplitMix64 finalizer.
fn mix(value: u64) -> u64 {
    let mut value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// UUID (v4 layout) for the entity `id` spawned at `tick` in a world with `seed`.
pub(crate) fn derive(seed: i64, tick: u64, id: EntityId) -> uuid::Uuid {
    let high = mix(mix(seed as u64) ^ tick);
    let low = mix(high ^ u64::from(id.0 as u32).wrapping_mul(0xd6e8_feb8_6659_fd93));
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&high.to_be_bytes());
    bytes[8..].copy_from_slice(&low.to_be_bytes());
    uuid::Builder::from_random_bytes(bytes).into_uuid()
}

impl TickState {
    /// UUID for an entity being spawned now with wire id `id`.
    pub(crate) fn entity_uuid(&self, id: EntityId) -> uuid::Uuid {
        derive(self.world.seed(), self.tick, id)
    }
}
