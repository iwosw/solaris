//! Native portal admission. Timers travel with entity custody, not connections.
use crate::boundary::BlockPos;
use crate::tick::TickState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "TimerImage")]
pub struct PortalState {
    pub elapsed: u32,
    pub cooldown: u16,
}

#[derive(Deserialize)]
struct TimerImage {
    #[serde(default)]
    elapsed: u32,
    #[serde(default)]
    cooldown: u16,
}

impl TryFrom<TimerImage> for PortalState {
    type Error = String;
    fn try_from(value: TimerImage) -> Result<Self, String> {
        if value.elapsed > i32::MAX as u32 || value.cooldown > 300 {
            return Err(format!(
                "portal timer: invalid elapsed {} or cooldown {} (maximum 2147483647/300)",
                value.elapsed, value.cooldown
            ));
        }
        Ok(Self {
            elapsed: value.elapsed,
            cooldown: value.cooldown,
        })
    }
}

pub(crate) fn state_id(state: &TickState, name: &str, props: &[(&str, &str)]) -> u32 {
    let name = mc_data::Identifier::parse(name).expect("vanilla block key");
    let default = state
        .registry
        .blocks
        .block(&name)
        .expect("vanilla block")
        .default;
    let mut properties = state
        .registry
        .blocks
        .by_id(default)
        .expect("default state")
        .properties
        .clone();
    for (key, value) in props {
        if let Some(property) = properties.iter_mut().find(|(k, _)| k == key) {
            property.1 = (*value).into();
        }
    }
    state
        .registry
        .blocks
        .by_name_and_props(&name, &properties)
        .expect("vanilla properties")
        .0
}

pub(crate) fn named(state: &TickState, pos: BlockPos, name: &str) -> bool {
    state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|block| block.block.id.path() == name)
}

pub(crate) fn write(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    pos: BlockPos,
    id: u32,
) {
    if state.world.block_at(pos.x, pos.y, pos.z) == Some(id) {
        return;
    }
    state.stage_container_removal(pos, true);
    state.world.set_block(pos.x, pos.y, pos.z, id);
    writes.push((pos, id));
}

pub(crate) fn commit_removals(state: &mut TickState, changes: &mut crate::boundary::ChangeSet) {
    let (_, mut events, spawned, removed) = state.commit_merge_effects();
    changes.entity_events.append(&mut events);
    changes.entities_spawned.extend(spawned);
    changes.entities_removed.extend(removed);
}

pub(crate) fn fields(timer: PortalState) -> Vec<(String, mc_nbt::Tag)> {
    vec![
        (
            "PortalCooldown".into(),
            mc_nbt::Tag::Int(i32::from(timer.cooldown)),
        ),
        (
            "solaris:PortalTime".into(),
            mc_nbt::Tag::Int(timer.elapsed as i32),
        ),
    ]
}

pub(crate) fn from_fields(
    fields: &[(String, mc_nbt::Tag)],
) -> Result<PortalState, crate::config::CoreError> {
    let mut image = TimerImage {
        elapsed: 0,
        cooldown: 0,
    };
    for key in ["PortalCooldown", "solaris:PortalTime"] {
        let entries: Vec<_> = fields.iter().filter(|(name, _)| name == key).collect();
        if entries.len() > 1 {
            return Err(crate::config::CoreError::Storage(format!(
                "portal timer: duplicate {key}"
            )));
        }
        if let Some((_, value)) = entries.first() {
            let mc_nbt::Tag::Int(value) = value else {
                return Err(crate::config::CoreError::Storage(format!(
                    "portal timer: {key} must be TAG_Int"
                )));
            };
            if *value < 0 || (key == "PortalCooldown" && *value > 300) {
                return Err(crate::config::CoreError::Storage(format!(
                    "portal timer: invalid {key} {value}"
                )));
            }
            if key == "PortalCooldown" {
                image.cooldown = *value as u16;
            } else {
                image.elapsed = *value as u32;
            }
        }
    }
    PortalState::try_from(image).map_err(crate::config::CoreError::Storage)
}

#[path = "portals_nether.rs"]
pub(crate) mod nether;
#[cfg(test)]
#[path = "portals_tests.rs"]
mod tests;
#[path = "portals_travel.rs"]
pub(crate) mod travel;
