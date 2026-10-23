use crate::boundary::EntityId;
use crate::entity_uuid::derive;

#[test]
fn derived_uuids_are_stable_v4_and_distinct() {
    let base = derive(42, 7, EntityId(9));
    assert_eq!(base, derive(42, 7, EntityId(9)));
    assert_eq!(base.get_version_num(), 4);
    assert_eq!(base.get_variant(), uuid::Variant::RFC4122);
    let mut seen = std::collections::HashSet::new();
    for seed in [42, -1] {
        for tick in 0..50 {
            for id in 1..50 {
                assert!(seen.insert(derive(seed, tick, EntityId(id))));
            }
        }
    }
}
