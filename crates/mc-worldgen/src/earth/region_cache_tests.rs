use super::super::definition::{DefinitionInput, PlaneBounds};
use super::super::region::RegionPlanner;
use super::super::world_plan::WorldPlan;
use super::*;
fn plans() -> (RegionPlan, RegionPlan) {
    let mut d = DefinitionInput::earth_root(42);
    d.domain = PlaneBounds {
        min_x: -128,
        max_x_exclusive: 128,
        min_z: -128,
        max_z_exclusive: 128,
    };
    d.macro_cell_m = 64.0;
    d.climate_cell_m = 128.0;
    let d = d.validate().unwrap();
    let world = WorldPlan::build(&d).unwrap();
    let identity = WorldIdentity::derive(&d, [1; 32], [2; 32]).unwrap();
    let planner = RegionPlanner::new(world.plane().unwrap(), identity);
    (
        planner
            .prepare(GeoNodeId {
                root: 5,
                level: 1,
                x: 0,
                z: 0,
            })
            .unwrap(),
        planner
            .prepare(GeoNodeId {
                root: 5,
                level: 1,
                x: 1,
                z: 0,
            })
            .unwrap(),
    )
}
#[test]
fn cache_eviction_reopen_and_clear_do_not_change_reconstructed_geometry() {
    let (a, b) = plans();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("derived.redb");
    let size_a = serde_json::to_vec(&a).unwrap().len() as u64 + 36;
    let size_b = serde_json::to_vec(&b).unwrap().len() as u64 + 36;
    let quota = size_a.max(size_b) + 1024;
    let cache = RegionCache::create(&path, quota, 1024 * 1024).unwrap();
    cache.insert(&a).unwrap();
    assert_eq!(cache.get(a.identity, a.node).unwrap(), Some(a.clone()));
    cache.insert(&b).unwrap();
    assert!(cache.get(a.identity, a.node).unwrap().is_none());
    assert!(cache.logical_bytes().unwrap() <= quota);
    drop(cache);
    let cache = RegionCache::open(&path, quota, 2 * 1024 * 1024).unwrap();
    assert_eq!(cache.get(b.identity, b.node).unwrap(), Some(b.clone()));
    cache.clear().unwrap();
    assert_eq!(cache.logical_bytes().unwrap(), 0);
    assert!(cache.get(b.identity, b.node).unwrap().is_none());
    cache.insert(&a).unwrap();
    assert_eq!(cache.get(a.identity, a.node).unwrap(), Some(a));
}
#[test]
fn cache_hash_and_version_corruption_are_misses_only_in_discardable_container() {
    let (a, _) = plans();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("derived.redb");
    let cache = RegionCache::create(&path, 16 * 1024 * 1024, 1024 * 1024).unwrap();
    cache.insert(&a).unwrap();
    let txn = cache.database.begin_write().unwrap();
    {
        let mut records = txn.open_table(RECORDS).unwrap();
        let k = key(a.identity, a.node);
        let mut bytes = records.get(k.as_slice()).unwrap().unwrap().value().to_vec();
        bytes[40] ^= 1;
        records.insert(k.as_slice(), bytes.as_slice()).unwrap();
    }
    txn.commit().unwrap();
    assert!(cache.get(a.identity, a.node).unwrap().is_none());
    cache.insert(&a).unwrap();
    let txn = cache.database.begin_write().unwrap();
    {
        let mut records = txn.open_table(RECORDS).unwrap();
        let k = key(a.identity, a.node);
        let mut bytes = records.get(k.as_slice()).unwrap().unwrap().value().to_vec();
        bytes[0] = REGION_VERSION as u8 + 1;
        records.insert(k.as_slice(), bytes.as_slice()).unwrap();
    }
    txn.commit().unwrap();
    assert!(cache.get(a.identity, a.node).unwrap().is_none());
    let other = WorldIdentity::derive(
        &DefinitionInput::earth_root(43).validate().unwrap(),
        [1; 32],
        [2; 32],
    )
    .unwrap();
    assert!(cache.get(other, a.node).unwrap().is_none());
    drop(cache);
    let native = dir.path().join("native.redb");
    let database = Database::create(&native).unwrap();
    drop(database);
    assert!(RegionCache::open(&native, 1024 * 1024, 1024 * 1024).is_err());
}
