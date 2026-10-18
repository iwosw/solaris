use super::{Gossip, conversion_tests::state, defense};
use crate::{
    boundary::{ConnectionId, PlayerInput},
    core::journal_entries_for_tick,
    tick::run_tick,
};
fn type_id(state: &crate::tick::TickState, name: &str) -> u32 {
    state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap()
}
#[test]
fn unchanged_villager_has_no_duplicate_journal_record() {
    let mut state = state();
    let id = state.spawn_mob(type_id(&state, "minecraft:villager"), [0., 80., 0.], 20.);
    let (output, _, _) = run_tick(&mut state, &[]);
    let first = journal_entries_for_tick(&mut state, &output);
    assert!(first.iter().any(|e|matches!(e,crate::journal::JournalEntry::Entity{id:entity,villager:Some(_),..} if *entity==id.0)));
    let (output, _, _) = run_tick(&mut state, &[]);
    assert!(
        !journal_entries_for_tick(&mut state, &output).iter().any(
            |e| matches!(e,crate::journal::JournalEntry::Entity{id:entity,..} if *entity==id.0)
        )
    );
    let slot = state.entities.slot_of(id).unwrap();
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .food_level = 12;
    assert!(journal_entries_for_tick(&mut state,&output).iter().any(|e|matches!(e,crate::journal::JournalEntry::Entity{id:entity,villager:Some(v),..} if *entity==id.0 && v.food_level==12)));
}
#[test]
fn iron_golem_avoids_creepers_and_friendly_players_but_defends_villagers() {
    let mut state = state();
    let connection = ConnectionId(91);
    let uuid = uuid::Uuid::from_u128(921);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection,
            uuid,
            name: "reputation".into(),
            permission_level: 0,
        }],
    );
    let player = state.connections.entity_of(connection).unwrap();
    let zombie = state.spawn_mob(type_id(&state, "minecraft:zombie"), [1., 80., 0.], 20.);
    let creeper = state.spawn_mob(type_id(&state, "minecraft:creeper"), [1., 80., 1.], 20.);
    let villager = state.spawn_mob(type_id(&state, "minecraft:villager"), [0., 80., 0.], 20.);
    let targets = defense::targets(&state);
    assert!(targets.iter().any(|(id, _)| *id == zombie));
    assert!(
        !targets
            .iter()
            .any(|(id, _)| *id == creeper || *id == player || *id == villager)
    );
    let slot = state.entities.slot_of(villager).unwrap();
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .gossips
        .push(Gossip {
            target: uuid,
            kind: "major_negative".into(),
            value: 20,
        });
    assert!(defense::targets(&state).iter().any(|(id, _)| *id == player));
}
