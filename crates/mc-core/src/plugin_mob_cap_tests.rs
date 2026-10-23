use super::*;

fn register(state: &mut TickState, id: &str, splitter: bool) -> u32 {
    let mut definition: serde_json::Value = serde_json::from_str(include_str!(
        "../../../sdk/rust/examples/sky-creeper/src/sky-creeper.json"
    ))
    .unwrap();
    definition["id"] = id.into();
    if splitter {
        definition["presents_as"] = "minecraft:slime".into();
        definition["capabilities"]["splits_on_death"] = true.into();
        definition["capabilities"]["size_profile"] = "slime".into();
    }
    Arc::get_mut(&mut state.registry)
        .unwrap()
        .register_blueprint(
            crate::blueprint::MobBlueprintDef::from_json(&definition.to_string()).unwrap(),
        )
        .unwrap()
}

#[test]
fn live_custom_cap_counts_defining_namespace_across_dimensions_and_releases_dead_rows() {
    let (mut root, _, position) = crate::hostile_tests::setup(12.0);
    let key = register(&mut root, "population:one", false);
    let second = register(&mut root, "population:two", false);
    let foreign = register(&mut root, "other:one", false);
    let (mut child, _, _) = crate::hostile_tests::setup(12.0);
    child.registry = Arc::clone(&root.registry);
    child.dimension = crate::dimensions::Dimension::Nether;
    for _ in 0..MAX_CUSTOM_MOBS_PER_PLUGIN / 2 {
        root.spawn_mob(key, position, 20.0);
        child.spawn_mob(second, position, 20.0);
    }
    root.other_dimensions
        .insert(crate::dimensions::Dimension::Nether, Box::new(child));
    assert!(custom_mob_cap_reached(&root, key));
    assert!(custom_mob_cap_reached(&root, second));
    assert!(!custom_mob_cap_reached(&root, foreign));
    let vanilla = root.registry.entity_type_id("minecraft:creeper").unwrap();
    assert!(!custom_mob_cap_reached(&root, vanilla));
    let slot = root
        .entities
        .slots()
        .find(|slot| {
            matches!(root.entities.body(*slot).kind,
        crate::entities::EntityKind::Mob { type_id } if type_id == key)
        })
        .unwrap();
    root.entities.body_mut(slot).alive = false;
    assert!(
        !custom_mob_cap_reached(&root, key),
        "death releases one live credit before row removal"
    );
}

#[test]
fn splitter_admission_reserves_maximum_descendants() {
    let (mut state, _, position) = crate::hostile_tests::setup(12.0);
    let ordinary = register(&mut state, "population:ordinary", false);
    let splitter = register(&mut state, "population:splitter", true);
    for _ in 0..MAX_CUSTOM_MOBS_PER_PLUGIN - 16 {
        state.spawn_mob(ordinary, position, 20.0);
    }
    assert!(!custom_mob_cap_reached(&state, splitter));
    state.spawn_mob(ordinary, position, 20.0);
    assert!(custom_mob_cap_reached(&state, splitter));
    assert!(!custom_mob_cap_reached(&state, ordinary));
}
