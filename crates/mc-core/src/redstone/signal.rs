//! Bounded one-block conduction. Weak power operates a consumer; only strong
//! power is relayed by a solid block. Wires suppress wire sources while settling
//! so a previously powered wire cannot sustain itself through its support.
use super::{at, blocks, containers, offset, prop, weak_output};
use crate::tick::TickState;
use mc_world::{BlockPos, BlockState};

pub(super) fn conductor(block: &BlockState) -> bool {
    let path = block.block.id.path();
    if matches!(path, "soul_sand" | "mud") {
        return true;
    }
    // Full collision alone is not sufficient: these full cubes are insulating.
    if path.ends_with("glass")
        || path.ends_with("_leaves")
        || path.ends_with("_shulker_box")
        || path.ends_with("copper_grate")
        || path.ends_with("copper_bulb")
        || matches!(
            path,
            "glass"
                | "shulker_box"
                | "ice"
                | "frosted_ice"
                | "glowstone"
                | "sea_lantern"
                | "beacon"
                | "honey_block"
                | "redstone_block"
                | "observer"
                | "piston"
                | "sticky_piston"
                | "moving_piston"
                | "tnt"
                | "spawner"
                | "trial_spawner"
        )
    {
        return false;
    }
    mc_data::collision_shapes::vanilla_collision_shapes()
        .get_for_state(block.id.0, &block.block.id, &block.properties)
        .is_some_and(|shape| shape.is_full_cube())
}

fn support(source: BlockPos, block: &BlockState) -> Option<BlockPos> {
    match prop(block, "face") {
        Some("floor") => offset(source, (0, -1, 0)),
        Some("ceiling") => offset(source, (0, 1, 0)),
        _ => {
            let d = prop(block, "facing").and_then(blocks::facing_delta)?;
            offset(source, (-d.0, -d.1, -d.2))
        }
    }
}

fn strong(state: &TickState, source: BlockPos, target: BlockPos, wires: bool) -> u8 {
    let Some(block) = at(state, source) else {
        return 0;
    };
    let path = block.block.id.path();
    let directed = match path {
        "redstone_wire" => wires,
        "repeater" | "comparator" | "observer" => true,
        "redstone_torch" | "redstone_wall_torch" => offset(source, (0, 1, 0)) == Some(target),
        "lever" => support(source, block) == Some(target),
        "detector_rail" | "trapped_chest" | "sculk_sensor" | "calibrated_sculk_sensor" => {
            offset(source, (0, -1, 0)) == Some(target)
        }
        _ if path.ends_with("_button") || path.ends_with("lightning_rod") => {
            support(source, block) == Some(target)
        }
        _ if path.ends_with("_pressure_plate") => offset(source, (0, -1, 0)) == Some(target),
        _ => false,
    };
    if directed {
        weak_output(state, source, target)
    } else {
        0
    }
}

pub(super) fn received(state: &TickState, source: BlockPos, target: BlockPos, wires: bool) -> u8 {
    let Some(block) = at(state, source) else {
        return 0;
    };
    let weak = if !wires && block.block.id.path() == "redstone_wire" {
        0
    } else {
        weak_output(state, source, target)
    };
    if weak == 15 || !conductor(block) {
        return weak;
    }
    blocks::NEIGHBOURS
        .into_iter()
        .filter_map(|d| offset(source, d))
        .map(|neighbor| strong(state, neighbor, source, wires))
        .max()
        .unwrap_or(0)
        .max(weak)
}

pub(super) fn dust(state: &BlockState, source: BlockPos, target: BlockPos) -> u8 {
    if target.y > source.y {
        return 0;
    }
    if target.y == source.y {
        let direction = blocks::HORIZONTAL
            .into_iter()
            .find(|(_, d)| offset(source, *d) == Some(target));
        if direction.is_none_or(|(name, _)| prop(state, name).is_none_or(|value| value == "none")) {
            return 0;
        }
    }
    prop(state, "power")
        .and_then(|p| p.parse().ok())
        .unwrap_or(0)
}

pub(super) fn torch(state: &BlockState, source: BlockPos, target: BlockPos) -> u8 {
    let attached = if state.block.id.path() == "redstone_torch" {
        offset(source, (0, -1, 0))
    } else {
        support(source, state)
    };
    if attached != Some(target) && prop(state, "lit") == Some("true") {
        15
    } else {
        0
    }
}

pub(super) fn analog(state: &TickState, pos: BlockPos) -> Option<u8> {
    let block = at(state, pos)?;
    let path = block.block.id.path();
    let number = |key| {
        prop(block, key)
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(0)
    };
    match path {
        "cauldron" => Some(0),
        "water_cauldron" | "powder_snow_cauldron" => Some(number("level")),
        "lava_cauldron" => Some(3),
        "composter" => Some(number("level")),
        "cake" => Some(14 - number("bites").min(6) * 2),
        "beehive" | "bee_nest" => Some(number("honey_level")),
        "sculk_sensor" | "calibrated_sculk_sensor" => crate::sculk::comparator(state, pos),
        "jukebox" => crate::jukebox::comparator(state, pos),
        _ if containers::has_analog_output(path) => Some(containers::signal(state, pos)),
        _ => None,
    }
}

pub(super) fn diode_input(state: &TickState, source: BlockPos, target: BlockPos) -> u8 {
    if let Some(block) = at(state, source)
        && block.block.id.path() == "redstone_wire"
    {
        return prop(block, "power")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
    }
    received(state, source, target, true)
}

pub(super) fn comparator_input(state: &TickState, pos: BlockPos, d: (i32, i32, i32)) -> u8 {
    let Some(back) = offset(pos, d) else {
        return 0;
    };
    if let Some(value) = analog(state, back) {
        return value;
    }
    let direct = diode_input(state, back, pos);
    if direct < 15
        && at(state, back).is_some_and(conductor)
        && let Some(value) = offset(back, d).and_then(|far| {
            analog(state, far).or_else(|| {
                let face = match d {
                    (1, 0, 0) => 5,
                    (-1, 0, 0) => 4,
                    (0, 0, 1) => 3,
                    _ => 2,
                };
                crate::item_frame::analog_output(state, far, face)
            })
        })
    {
        return value;
    }
    direct
}

pub(super) fn daylight(state: &TickState, pos: BlockPos, inverted: bool) -> u8 {
    use crate::dimensions::Dimension;
    if state.dimension == Dimension::Nether {
        return if inverted { 15 } else { 0 };
    }
    let sky = state
        .world
        .light_at(pos.x, pos.y, pos.z)
        .map_or(0, |(sky, _)| sky);
    let time = if state.dimension == Dimension::End {
        6000
    } else {
        state.day_time
    };
    let darken = if state.dimension == Dimension::End {
        0
    } else {
        crate::light::sky_darken(time, state.rain_level, state.thunder_level)
    };
    let light = sky.saturating_sub(darken);
    if inverted {
        return 15 - light;
    }
    if state.dimension == Dimension::End {
        return 0;
    }
    let phase = (time.rem_euclid(24000) as f64 / 24000.0 - 0.25).rem_euclid(1.0);
    let eased = (1.0 - f64::from(mc_physics::mth::cos(phase * std::f64::consts::PI))) / 2.0;
    let angle = (phase + (eased - phase) / 3.0) * std::f64::consts::TAU;
    let noon = if angle < std::f64::consts::PI {
        0.0
    } else {
        std::f64::consts::TAU
    };
    let angle = angle + (noon - angle) * 0.2;
    (f64::from(light) * f64::from(mc_physics::mth::cos(angle)))
        .round()
        .clamp(0.0, 15.0) as u8
}

pub(super) fn connects(block: &BlockState, direction: (i32, i32, i32)) -> bool {
    let path = block.block.id.path();
    match path {
        "repeater" => prop(block, "facing")
            .and_then(blocks::facing_delta)
            .is_some_and(|d| d == direction || d == (-direction.0, 0, -direction.2)),
        "observer" => prop(block, "facing").and_then(blocks::facing_delta) == Some(direction),
        "redstone_wire"
        | "comparator"
        | "redstone_torch"
        | "redstone_wall_torch"
        | "lever"
        | "redstone_block"
        | "daylight_detector"
        | "detector_rail"
        | "trapped_chest"
        | "target"
        | "jukebox"
        | "sculk_sensor"
        | "calibrated_sculk_sensor" => true,
        _ => {
            path.ends_with("_button")
                || path.ends_with("_pressure_plate")
                || path.ends_with("lightning_rod")
        }
    }
}

pub(super) fn comparator_side(state: &TickState, source: BlockPos, target: BlockPos) -> u8 {
    let Some(block) = at(state, source) else {
        return 0;
    };
    match block.block.id.path() {
        "redstone_block" => 15,
        "redstone_wire" => prop(block, "power")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        "repeater" | "comparator" => weak_output(state, source, target),
        _ => 0,
    }
}
