use super::*;
use mc_worldgen::vanilla_features::{LegacyRandom, RandomSource};

pub(super) fn shuffle<const N: usize>(seed: i64) -> [u8; N] {
    let mut order = std::array::from_fn(|i| i as u8);
    let mut random = LegacyRandom::new(seed);
    for i in (1..N).rev() {
        let j = random.next_int_bounded((i + 1) as i32) as usize;
        order.swap(i, j);
    }
    order
}
fn rank(state: &TickState, index: u8) -> i32 {
    let mut random = LegacyRandom::new(state.world.seed());
    let seed = random.next_long() & 65535;
    i32::from(shuffle::<10>(seed)[usize::from(index)])
}
/// Vanilla `SpikeFeature` centers: `floor(42·cos/sin(2·(−π + π/10·i)))` in
/// `java.lang.Math`. Pinned as constants because world layout must match
/// imported vanilla Ends exactly (the `Mth` sine table puts spike 5 at z=0).
const SPIKE_CENTERS: [(i8, i8); 10] = [
    (42, 0),
    (33, 24),
    (12, 39),
    (-13, 39),
    (-34, 24),
    (-42, -1),
    (-34, -25),
    (-13, -40),
    (12, -40),
    (33, -25),
];
pub(super) fn position(state: &TickState, index: u8) -> [f64; 3] {
    let (x, z) = SPIKE_CENTERS[usize::from(index)];
    [
        f64::from(x) + 0.5,
        f64::from(77 + rank(state, index) * 3),
        f64::from(z) + 0.5,
    ]
}
pub(super) fn ready(state: &TickState) -> bool {
    (0..10).all(|index| {
        let [x, _, z] = position(state, index);
        let x = x.floor() as i32;
        let z = z.floor() as i32;
        state.world.block_at(x - 10, 64, z - 10).is_some()
            && state.world.block_at(x + 10, 64, z + 10).is_some()
    }) && state.world.block_at(0, 64, 0).is_some()
}
pub(super) fn write_state(
    state: &mut TickState,
    changes: &mut ChangeSet,
    at: [i32; 3],
    block: u32,
) {
    let [x, y, z] = at;
    if state.world.block_at(x, y, z) == Some(block) {
        return;
    }
    state.world.set_block(x, y, z, block);
    changes.blocks.push(crate::boundary::BlockChange {
        position: crate::boundary::BlockPos { x, y, z },
        state: block,
    });
}
pub(super) fn write_block(
    state: &mut TickState,
    changes: &mut ChangeSet,
    at: [i32; 3],
    name: &str,
) {
    let id = mc_data::Identifier::parse(name).expect("canonical fight block");
    let block = state
        .registry
        .blocks
        .block(&id)
        .expect("vanilla fight block")
        .default
        .0;
    write_state(state, changes, at, block);
}
pub(super) fn build(state: &mut TickState, changes: &mut ChangeSet, index: u8) {
    let [px, py, pz] = position(state, index);
    let x = px.floor() as i32;
    let z = pz.floor() as i32;
    let height = py as i32 - 1;
    let rank = rank(state, index);
    let radius = 2 + rank / 3;
    let air = state.registry.states.air;
    let obsidian = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:obsidian").unwrap())
        .unwrap()
        .default
        .0;
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            for y in 0..=height + 10 {
                if dx * dx + dz * dz <= radius * radius + 1 && y < height {
                    write_state(state, changes, [x + dx, y, z + dz], obsidian);
                } else if y > 65 {
                    write_state(state, changes, [x + dx, y, z + dz], air);
                }
            }
        }
    }
    if rank == 1 || rank == 2 {
        for dx in -2i32..=2 {
            for dz in -2i32..=2 {
                for dy in 0..=3 {
                    let roof = dy == 3;
                    if dx.abs() != 2 && dz.abs() != 2 && !roof {
                        continue;
                    }
                    let along_z = dx.abs() == 2 || roof;
                    let along_x = dz.abs() == 2 || roof;
                    let properties = [
                        ("north", along_z && dz != -2),
                        ("south", along_z && dz != 2),
                        ("west", along_x && dx != -2),
                        ("east", along_x && dx != 2),
                    ];
                    let id = mc_data::Identifier::parse("minecraft:iron_bars").unwrap();
                    let props: Vec<_> = properties
                        .iter()
                        .map(|(k, v)| ((*k).to_string(), v.to_string()))
                        .chain(std::iter::once(("waterlogged".into(), "false".into())))
                        .collect();
                    let block = state
                        .registry
                        .blocks
                        .by_name_and_props(&id, &props)
                        .expect("vanilla iron-bar states")
                        .0;
                    write_state(state, changes, [x + dx, height + dy, z + dz], block);
                }
            }
        }
    }
    write_block(state, changes, [x, height, z], "minecraft:bedrock");
    write_block(state, changes, [x, height + 1, z], "minecraft:fire");
    let id = state.spawn_mob(type_id(state, "minecraft:end_crystal"), [px, py, pz], 1.0);
    let slot = state.entities.slot_of(id).expect("crystal spawned");
    let uuid = state.entities.row(slot).uuid;
    let respawning = state.end_fight.dragon_killed;
    state.end_fight.crystals.push(Crystal {
        uuid,
        position: [px, py, pz],
        pillar: Some(index),
        beam: respawning.then_some([0, 128, 0]),
        invulnerable: respawning,
    });
    changes.entities_spawned.push(id);
}
pub(super) fn rebuild(state: &mut TickState, changes: &mut ChangeSet, index: u8) {
    let old: Vec<_> = state
        .end_fight
        .crystals
        .iter()
        .filter(|c| c.pillar == Some(index))
        .map(|c| c.uuid)
        .collect();
    for uuid in &old {
        if let Some(slot) = state.entities.slot_of_uuid(*uuid) {
            let id = state.entities.body(slot).id;
            state.remove_entity(slot);
            changes.entities_removed.push(id);
        }
    }
    state.end_fight.crystals.retain(|c| !old.contains(&c.uuid));
    let [px, py, pz] = position(state, index);
    let x = px.floor() as i32;
    let z = pz.floor() as i32;
    let y = py as i32 - 1;
    let air = state.registry.states.air;
    for dx in -10..=10 {
        for dz in -10..=10 {
            for dy in -10..=10 {
                write_state(state, changes, [x + dx, y + dy, z + dz], air);
            }
        }
    }
    crate::explosion::apply_no_blocks(state, [px, f64::from(y), pz], 5.0, &[]);
    build(state, changes, index);
}
