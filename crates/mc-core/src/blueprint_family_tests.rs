use crate::blueprint::MobBlueprintDef;
use crate::tick::TickState;
use std::sync::Arc;

fn register(
    state: &mut TickState,
    name: &str,
    presentation: &str,
    capabilities: serde_json::Value,
) -> u32 {
    let definition = serde_json::json!({
        "id": name, "presents_as": presentation, "category": "creature",
        "movement": "walker", "goals": [{"priority": 0, "kind": "idle"}],
        "sight_range": 16.0, "attack_reach": 2.0, "max_health": 20.0,
        "speed": 2.0, "attack_damage": 2.0, "loot_table": "minecraft:entities/creeper",
        "capabilities": capabilities,
    });
    Arc::get_mut(&mut state.registry)
        .unwrap()
        .register_blueprint(MobBlueprintDef::from_json(&definition.to_string()).unwrap())
        .unwrap()
}

#[test]
fn custom_creeper_opts_into_pet_breeding_and_loadout_without_wolf_metadata() {
    let (mut state, _, position) = crate::hostile_tests::setup(12.0);
    let key = register(
        &mut state,
        "families:pet",
        "minecraft:creeper",
        serde_json::json!({
            "taming": "bone_pet", "food": "wolf", "breeding": "tamed_standing",
            "leashable": true, "ageable": true, "equipment": "bow_armor",
        }),
    );
    let id = state.spawn_mob(key, position, 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    assert!(!crate::entity_life::can_breed(
        &state,
        state.entities.body(slot),
        state.entities.row(slot)
    ));
    let bow = state.entities.row(slot).appearance.equipment[0]
        .as_ref()
        .expect("custom loadout equips bow");
    assert_eq!(
        state.registry.items.name_of(bow.item_id).unwrap().as_str(),
        "minecraft:bow"
    );
    let owner = uuid::Uuid::from_u128(123);
    crate::taming::apply(&mut state, slot, owner, &crate::taming::Action::Tame(1));
    let row = state.entities.row(slot);
    assert!(row.entity_life.tamed);
    assert_eq!(row.entity_life.taming.owner, Some(owner));
    assert_eq!(
        row.health, 40.0,
        "bone-pet health policy is not a wolf type check"
    );
    assert!(row.entity_life.sitting, "newly tamed pets sit");
    assert!(!crate::entity_life::can_breed(
        &state,
        state.entities.body(slot),
        row
    ));
    crate::taming::apply(&mut state, slot, owner, &crate::taming::Action::Sit);
    let row = state.entities.row(slot);
    assert!(crate::entity_life::can_breed(
        &state,
        state.entities.body(slot),
        row
    ));
    assert!(
        !row.appearance.extra.iter().any(|(_, value)| matches!(
            value,
            crate::boundary::EntityDataValue::OptionalLivingEntityReference(_)
        )),
        "wolf metadata must not enter creeper layout"
    );
    state.entities.row_mut(slot).entity_life.sitting = true;
    assert!(!crate::entity_life::can_breed(
        &state,
        state.entities.body(slot),
        state.entities.row(slot)
    ));
}

#[test]
fn slime_presentation_does_not_grant_size_or_split_behavior() {
    let (mut state, _, position) = crate::hostile_tests::setup(12.0);
    let plain = register(
        &mut state,
        "families:plain",
        "minecraft:slime",
        serde_json::json!({}),
    );
    let sized = register(
        &mut state,
        "families:sized",
        "minecraft:slime",
        serde_json::json!({
            "size_profile": "slime", "splits_on_death": true,
        }),
    );
    let plain = state.spawn_mob(plain, position, 20.0);
    let sized = state.spawn_mob(sized, position, 20.0);
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(plain).unwrap())
            .loot
            .slime_size,
        None
    );
    assert!(
        state
            .entities
            .row(state.entities.slot_of(sized).unwrap())
            .loot
            .slime_size
            .is_some()
    );
}

#[test]
fn custom_definition_change_changes_world_identity() {
    let (mut state, _, _) = crate::hostile_tests::setup(12.0);
    let before = crate::plugin_startup::world_identity(&state.registry).unwrap();
    let key = register(
        &mut state,
        "families:pinned",
        "minecraft:creeper",
        serde_json::json!({"leashable": true}),
    );
    let admitted = crate::plugin_startup::world_identity(&state.registry).unwrap();
    assert_ne!(before, admitted);
    Arc::get_mut(&mut state.registry)
        .unwrap()
        .blueprints
        .iter_mut()
        .find(|(id, _)| *id == key)
        .unwrap()
        .1
        .capabilities
        .leashable = false;
    assert_ne!(
        admitted,
        crate::plugin_startup::world_identity(&state.registry).unwrap()
    );
}
