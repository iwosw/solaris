use super::*;
use mc_data::blocks::{BlockReport, BlockStateReport};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn workspace_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join(rel)
}

type StateSpec<'a> = (u32, bool, &'a [(&'a str, &'a str)]);

fn report(id: &str, props: &[(&str, &[&str])], states: &[StateSpec<'_>]) -> BlockReport {
    BlockReport {
        id: Identifier::parse(id).unwrap(),
        properties: props
            .iter()
            .map(|(n, vs)| {
                (
                    (*n).to_string(),
                    vs.iter().map(|s| (*s).to_string()).collect(),
                )
            })
            .collect(),
        states: states
            .iter()
            .map(|(id, default, ps)| BlockStateReport {
                id: *id,
                default: *default,
                properties: ps
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect::<BTreeMap<_, _>>(),
            })
            .collect(),
    }
}

#[test]
fn builds_from_minimal_report() {
    // Dense ids 0..5 — real reports are dense and the registry
    // enforces that invariant.
    let r = vec![
        report("minecraft:air", &[], &[(0, true, &[])]),
        report("minecraft:stone", &[], &[(1, true, &[])]),
        report(
            "minecraft:oak_log",
            &[("axis", &["x", "y", "z"])],
            &[
                (2, false, &[("axis", "x")]),
                (3, true, &[("axis", "y")]),
                (4, false, &[("axis", "z")]),
            ],
        ),
    ];
    let reg = BlockRegistry::from_report(&r).unwrap();
    assert_eq!(reg.len(), 5);

    let air_id = Identifier::parse("minecraft:air").unwrap();
    assert_eq!(reg.block(&air_id).unwrap().default, BlockStateId(0));
    assert_eq!(reg.by_id(BlockStateId(0)).unwrap().block.id, air_id);
    assert_eq!(reg.by_id(BlockStateId(0)).unwrap().block.raw_id, 0);
    assert_eq!(reg.by_id(BlockStateId(1)).unwrap().block.raw_id, 1);

    let oak = Identifier::parse("minecraft:oak_log").unwrap();
    assert_eq!(
        reg.by_name_and_props(&oak, &[("axis".into(), "y".into())]),
        Some(BlockStateId(3))
    );
    // Property ordering shouldn't matter to the caller.
    assert_eq!(
        reg.by_name_and_props(&oak, &[("axis".into(), "z".into())]),
        Some(BlockStateId(4))
    );
}

#[test]
fn rejects_duplicate_state_id() {
    let r = vec![
        report("minecraft:air", &[], &[(0, true, &[])]),
        report("minecraft:stone", &[], &[(0, true, &[])]),
    ];
    assert!(matches!(
        BlockRegistry::from_report(&r),
        Err(RegistryError::DuplicateState(_))
    ));
}

#[test]
fn rejects_missing_default() {
    let r = vec![report("minecraft:air", &[], &[(0, false, &[])])];
    assert!(matches!(
        BlockRegistry::from_report(&r),
        Err(RegistryError::NoDefault(_))
    ));
}

/// Exhaustive opt-in round-trip on the local vanilla sidecar.
#[test]
#[ignore = "requires local 26.1.2 data/vanilla blocks report"]
fn round_trip_real_blocks_report() {
    let path = workspace_path("data/vanilla/reports/blocks.json");
    assert!(
        path.is_file(),
        "{} not present; run tools/extract-vanilla-data.sh",
        path.display()
    );
    let report = mc_data::blocks::load_blocks_report(&path).unwrap();
    let reg = BlockRegistry::from_report(&report).unwrap();

    // Cardinals pinned to 26.1.2.
    assert_eq!(reg.len(), 35723);
    let air = Identifier::parse("minecraft:air").unwrap();
    assert_eq!(reg.block(&air).unwrap().default, BlockStateId(0));

    // Every state survives an id → (name, props) → id round-trip.
    for state in reg.states() {
        let same = reg
            .by_name_and_props(&state.block.id, &state.properties)
            .expect("registered state must round-trip");
        assert_eq!(same, state.id);
    }
}
