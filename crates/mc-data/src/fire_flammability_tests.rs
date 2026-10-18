use super::odds;

#[test]
fn embedded_odds_match_all_207_measured_bootstrap_registrations() {
    let report = crate::blocks::solaris_required_blocks_report();
    let mut registered: Vec<_> = report
        .iter()
        .filter(|b| odds(b.id.as_str()) != (0, 0))
        .collect();
    registered.sort_by_key(|b| b.id.path());
    assert_eq!(registered.len(), 207);
    // FNV-1a over sorted bootstrap block paths followed by ignite/burn bytes.
    // Expected value is measured from FireBlock.bootStrap, independently of this lookup.
    let mut fingerprint = 0xcbf29ce484222325u64;
    for block in registered {
        let (ignite, burn) = odds(block.id.as_str());
        for byte in block.id.path().bytes().chain([ignite, burn]) {
            fingerprint = (fingerprint ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    assert_eq!(fingerprint, 0x859bc5da84da7e17);
}

#[test]
fn partial_wood_and_carpet_use_exact_odds_without_inventing_suffix_fuel() {
    assert_eq!(odds("minecraft:oak_stairs"), (5, 20));
    assert_eq!(odds("oak_slab"), (5, 20));
    assert_eq!(odds("oak_fence"), (5, 20));
    assert_eq!(odds("oak_fence_gate"), (5, 20));
    assert_eq!(odds("white_carpet"), (60, 20));
    assert_eq!(odds("vine"), (15, 100));
    assert_eq!(odds("scaffolding"), (60, 60));
    assert_eq!(odds("white_wool"), (30, 60));
    for block in [
        "stone",
        "oak_door",
        "crimson_planks",
        "warped_planks",
        "warped_stairs",
        "minecraft:unknown",
        "other:oak_stairs",
    ] {
        assert_eq!(odds(block), (0, 0), "{block}");
    }
}
