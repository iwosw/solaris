use super::vanilla_outline_shapes;
use crate::blocks::solaris_required_blocks_report;

#[test]
fn outline_table_covers_every_fingerprinted_vanilla_state() {
    let table = vanilla_outline_shapes();
    let blocks = solaris_required_blocks_report();
    let mut count = 0;
    for block in &blocks {
        for state in &block.states {
            let properties: Vec<_> = state
                .properties
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            assert!(
                table
                    .get_for_state(state.id, &block.id, &properties)
                    .is_some(),
                "missing outline {} state {}",
                block.id,
                state.id
            );
            count += 1;
        }
    }
    assert_eq!(count, 35_723);
    assert_eq!(table.covered_state_count(), count);
}

#[test]
fn outline_table_distinguishes_torch_and_low_fire_from_collision_and_full_voxels() {
    let blocks = solaris_required_blocks_report();
    let state = |name: &str| {
        blocks
            .iter()
            .find(|block| block.id.as_str() == name)
            .unwrap()
    };
    let torch = &state("minecraft:torch").states[0];
    let boxes: Vec<_> = vanilla_outline_shapes()
        .get(torch.id)
        .unwrap()
        .iter()
        .map(|bounds| bounds.coordinates())
        .collect();
    assert_eq!(boxes, vec![[1536, 0, 1536, 2560, 2560, 2560]]);
    let fire = state("minecraft:fire")
        .states
        .iter()
        .find(|state| {
            ["north", "south", "east", "west", "up"].iter().all(|key| {
                state
                    .properties
                    .get(*key)
                    .is_some_and(|value| value == "false")
            })
        })
        .unwrap();
    let boxes: Vec<_> = vanilla_outline_shapes()
        .get(fire.id)
        .unwrap()
        .iter()
        .map(|bounds| bounds.coordinates())
        .collect();
    assert_eq!(boxes, vec![[0, 0, 0, 4096, 256, 4096]]);
}

#[test]
fn outline_table_refuses_mismatched_state_identity() {
    let blocks = solaris_required_blocks_report();
    let torch = blocks
        .iter()
        .find(|block| block.id.as_str() == "minecraft:torch")
        .unwrap();
    let stone = blocks
        .iter()
        .find(|block| block.id.as_str() == "minecraft:stone")
        .unwrap();
    assert!(
        vanilla_outline_shapes()
            .get_for_state(torch.states[0].id, &stone.id, &[])
            .is_none()
    );
}
