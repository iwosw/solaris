//! Authoritative baked-light transport and per-viewer reconciliation.
//!
//! Both chunk loads and incremental updates use the world's one wire encoder.
//! A baseline owns only section descriptors; their nibble allocations remain
//! shared (COW). It advances only after a packet fits the world queue, so a
//! deferred update is encoded from the newest snapshot, including zero masks
//! that clear light after an emitter disappears.

use mc_core::boundary::ConnectionId;
use mc_protocol::packets::play::{GameEvent, LightData, LightUpdate};
use mc_world::chunk::{Chunk, SectionLight};
use rustc_hash::FxHashMap;

use crate::plan::{PlannedPacket, push_best_effort, push_reliable, try_push};

/// Encode authoritative baked light; refuse an unbaked snapshot without panic.
pub(crate) fn chunk_light(chunk: &Chunk) -> LightData {
    let Some(baked) = mc_world::light::ChunkLight::from_chunk(chunk) else {
        tracing::warn!(
            chunk_x = chunk.pos.x,
            chunk_z = chunk.pos.z,
            "unbaked core snapshot: sending empty wire light"
        );
        return LightData::empty();
    };
    let wire = mc_world::wire::encode_chunk_light(&baked);
    LightData {
        sky_y_mask: wire.sky_y_mask,
        block_y_mask: wire.block_y_mask,
        empty_sky_y_mask: wire.empty_sky_y_mask,
        empty_block_y_mask: wire.empty_block_y_mask,
        sky_updates: wire.sky_updates,
        block_updates: wire.block_updates,
    }
}

#[derive(Debug, Default)]
pub(crate) struct LightBaseline {
    sent: FxHashMap<(i32, i32), LightState>,
}

#[derive(Debug, Default)]
struct LightState {
    sent: Vec<SectionLight>,
    observed: Vec<SectionLight>,
}

impl LightBaseline {
    pub(crate) fn changed(&self, pos: (i32, i32), chunk: &Chunk) -> bool {
        self.sent
            .get(&pos)
            .is_some_and(|state| state.sent != chunk.section_lights)
    }

    pub(crate) fn record(&mut self, pos: (i32, i32), chunk: &Chunk) {
        let state = self.sent.entry(pos).or_default();
        state.sent.clone_from(&chunk.section_lights);
        state.observed.clone_from(&chunk.section_lights);
    }

    /// Compare against the last observed revision, not the last delivery:
    /// a credit-starved torch placement/removal may return to the sent arrays.
    pub(crate) fn observe(&mut self, pos: (i32, i32), chunk: &Chunk) -> bool {
        let Some(state) = self.sent.get_mut(&pos) else {
            return false;
        };
        let changed = state.observed != chunk.section_lights;
        if changed {
            state.observed.clone_from(&chunk.section_lights);
        }
        changed
    }

    pub(crate) fn forget(&mut self, pos: (i32, i32)) {
        self.sent.remove(&pos);
    }

    pub(crate) fn retain_held(&mut self, held: &FxHashMap<(i32, i32), u64>) {
        self.sent.retain(|pos, _| held.contains_key(pos));
    }

    pub(crate) fn update(
        &mut self,
        pos: (i32, i32),
        chunk: &Chunk,
        connection: ConnectionId,
        out: &mut Vec<PlannedPacket>,
        credit: &mut usize,
    ) {
        if !self.changed(pos, chunk) || *credit == 0 {
            return;
        }
        if try_push(out, credit, |out| {
            push_best_effort(
                out,
                connection,
                &LightUpdate {
                    chunk_x: pos.0,
                    chunk_z: pos.1,
                    light: chunk_light(chunk),
                },
            );
        }) {
            self.record(pos, chunk);
        }
    }
}

/// Normal fades send changed levels first. Crossing 20% rain sends the
/// vanilla state event and full level snapshot afterwards (including duplicates).
#[derive(Debug, Default)]
pub(crate) struct WeatherBaseline {
    sent: Option<(bool, f32, f32)>,
    force_state_sync: bool,
}

impl WeatherBaseline {
    pub(crate) fn reset(&mut self) {
        self.sent = None;
        self.force_state_sync = true;
    }

    pub(crate) fn snapshot(
        &mut self,
        connection: ConnectionId,
        raining: bool,
        rain: f32,
        thunder: f32,
        out: &mut Vec<PlannedPacket>,
    ) {
        // Login's dry defaults already describe a newly connected dry viewer.
        if self.sent.is_none()
            && !self.force_state_sync
            && !raining
            && rain == 0.0
            && thunder == 0.0
        {
            self.sent = Some((false, 0.0, 0.0));
            return;
        }
        self.update(connection, raining, rain, thunder, out);
    }

    pub(crate) fn update(
        &mut self,
        connection: ConnectionId,
        raining: bool,
        rain: f32,
        thunder: f32,
        out: &mut Vec<PlannedPacket>,
    ) {
        let Some(previous) = self.sent else {
            self.state_sync(connection, raining, rain, thunder, out);
            self.force_state_sync = false;
            self.sent = Some((raining, rain, thunder));
            return;
        };
        for (event, value, previous) in [
            (GameEvent::EVENT_RAIN_LEVEL_CHANGE, rain, previous.1),
            (GameEvent::EVENT_THUNDER_LEVEL_CHANGE, thunder, previous.2),
        ] {
            if previous.to_bits() != value.to_bits() {
                push_reliable(out, connection, &GameEvent { event, value });
            }
        }
        if (raining && previous.1 < 0.2 && rain >= 0.2)
            || (!raining && previous.1 >= 0.2 && rain < 0.2)
        {
            self.state_sync(connection, raining, rain, thunder, out);
        }
        self.sent = Some((raining, rain, thunder));
    }

    fn state_sync(
        &self,
        connection: ConnectionId,
        raining: bool,
        rain: f32,
        thunder: f32,
        out: &mut Vec<PlannedPacket>,
    ) {
        for (event, value) in [
            (
                if raining {
                    GameEvent::EVENT_START_RAINING
                } else {
                    GameEvent::EVENT_STOP_RAINING
                },
                0.0,
            ),
            (GameEvent::EVENT_RAIN_LEVEL_CHANGE, rain),
            (GameEvent::EVENT_THUNDER_LEVEL_CHANGE, thunder),
        ] {
            push_reliable(out, connection, &GameEvent { event, value });
        }
    }
}

#[cfg(test)]
#[path = "light_tests.rs"]
mod light_tests;
