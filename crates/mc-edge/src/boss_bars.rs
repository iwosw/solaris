//! Boss bars are snapshot baselines, not one-shot events: coalesced ticks and
//! returning viewers cannot strand an old progress value or a dead boss bar.
use crate::plan::{PlannedPacket, push_reliable};
use mc_core::boundary::{ConnectionId, EntityDataValue, EntityView};
use mc_protocol::packets::play::{BossOperation, ClientboundBossEvent};
use rustc_hash::FxHashMap;
use uuid::Uuid;

#[derive(Debug, Default)]
pub(crate) struct BossBaseline {
    known: FxHashMap<Uuid, f32>,
    raids: FxHashMap<Uuid, (f32, &'static str)>,
}
impl BossBaseline {
    pub(crate) fn sync<'a>(
        &mut self,
        connection: ConnectionId,
        visible: impl Iterator<Item = &'a EntityView> + Clone,
        packets: &mut Vec<PlannedPacket>,
    ) {
        let is_wither = |entity: &EntityView| {
            mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(entity.type_id)
                .is_some_and(|contract| match contract.name {
                    "minecraft:wither" => entity.appearance.health > 0.0,
                    "minecraft:ender_dragon" => true,
                    _ => false,
                })
        };
        self.known.retain(|uuid, _| {
            let retained = visible
                .clone()
                .any(|entity| entity.uuid == *uuid && is_wither(entity));
            if !retained {
                push_reliable(
                    packets,
                    connection,
                    &ClientboundBossEvent {
                        uuid: *uuid,
                        operation: BossOperation::Remove,
                    },
                );
            }
            retained
        });
        for entity in visible.filter(|entity| is_wither(entity)) {
            let dragon =
                mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(entity.type_id)
                    .is_some_and(|c| c.name == "minecraft:ender_dragon");
            let invulnerable = entity
                .appearance
                .extra
                .iter()
                .find_map(|(index, value)| match (*index, value) {
                    (19, EntityDataValue::VarInt(value)) => Some(*value),
                    _ => None,
                })
                .unwrap_or(0);
            let progress = if dragon {
                entity.appearance.health / 200.0
            } else if invulnerable > 0 {
                1.0 - invulnerable as f32 / 220.0
            } else {
                entity.appearance.health / 300.0
            }
            .clamp(0.0, 1.0);
            let operation = match self.known.get(&entity.uuid) {
                None => BossOperation::Add {
                    name: mc_nbt::Tag::Compound(vec![(
                        "translate".into(),
                        mc_nbt::Tag::String(
                            if dragon {
                                "entity.minecraft.ender_dragon"
                            } else {
                                "entity.minecraft.wither"
                            }
                            .into(),
                        ),
                    )]),
                    progress,
                    color: if dragon { 0 } else { 5 },
                    overlay: 0,
                    flags: if dragon { 6 } else { 1 },
                },
                Some(old) if *old != progress => BossOperation::Progress(progress),
                _ => continue,
            };
            push_reliable(
                packets,
                connection,
                &ClientboundBossEvent {
                    uuid: entity.uuid,
                    operation,
                },
            );
            self.known.insert(entity.uuid, progress);
        }
    }

    pub(crate) fn sync_raids(
        &mut self,
        connection: ConnectionId,
        position: [f64; 3],
        bars: &[mc_core::raids::RaidBar],
        packets: &mut Vec<PlannedPacket>,
    ) {
        let in_range = |bar: &mc_core::raids::RaidBar| {
            position
                .iter()
                .zip(bar.center)
                .map(|(a, b)| (a - f64::from(b)).powi(2))
                .sum::<f64>()
                < 9216.0
        };
        self.raids.retain(|uuid, _| {
            let retained = bars.iter().any(|bar| bar.uuid == *uuid && in_range(bar));
            if !retained {
                push_reliable(
                    packets,
                    connection,
                    &ClientboundBossEvent {
                        uuid: *uuid,
                        operation: BossOperation::Remove,
                    },
                );
            }
            retained
        });
        for bar in bars.iter().filter(|bar| in_range(bar)) {
            let name = || {
                mc_nbt::Tag::Compound(vec![(
                    "translate".into(),
                    mc_nbt::Tag::String(bar.name.into()),
                )])
            };
            let progress = bar.progress.clamp(0.0, 1.0);
            if let Some(&(old_progress, old_name)) = self.raids.get(&bar.uuid) {
                if old_progress != progress {
                    push_reliable(
                        packets,
                        connection,
                        &ClientboundBossEvent {
                            uuid: bar.uuid,
                            operation: BossOperation::Progress(progress),
                        },
                    );
                }
                if old_name != bar.name {
                    push_reliable(
                        packets,
                        connection,
                        &ClientboundBossEvent {
                            uuid: bar.uuid,
                            operation: BossOperation::Name(name()),
                        },
                    );
                }
            } else {
                // Raid constructor: RED=2, NOTCHED_10=2, no environmental flags.
                push_reliable(
                    packets,
                    connection,
                    &ClientboundBossEvent {
                        uuid: bar.uuid,
                        operation: BossOperation::Add {
                            name: name(),
                            progress,
                            color: 2,
                            overlay: 2,
                            flags: 0,
                        },
                    },
                );
            }
            self.raids.insert(bar.uuid, (progress, bar.name));
        }
    }
}
#[cfg(test)]
#[path = "boss_bars_tests.rs"]
mod tests;
