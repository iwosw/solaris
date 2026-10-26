//! Vanilla death components and server-side team broadcast visibility.
use crate::boundary::{EntityId, PlayerChange};
use crate::intent::DamageSource;
use crate::survival::DamageKind;
use crate::tick::TickState;
use mc_nbt::{ListTag, Tag};

pub(super) fn component(
    state: &TickState,
    victim: EntityId,
    kind: DamageKind,
    source: Option<DamageSource>,
) -> Tag {
    let attacker = crate::statistics::attacker(source);
    let type_name = kind.type_name(
        source,
        attacker.is_some_and(|id| state.players.contains_key(&id)),
    );
    // message_id values from the pinned 26.1.2 damage_type data.
    let message_id = match type_name
        .strip_prefix("minecraft:")
        .expect("vanilla damage identity")
    {
        "generic_kill" => "genericKill",
        "out_of_world" => "outOfWorld",
        "in_wall" => "inWall",
        "in_fire" | "campfire" => "inFire",
        "on_fire" => "onFire",
        "hot_floor" => "hotFloor",
        "sweet_berry_bush" => "sweetBerryBush",
        "falling_anvil" => "anvil",
        "falling_stalactite" => "fallingStalactite",
        "lightning_bolt" => "lightningBolt",
        "fly_into_wall" => "flyIntoWall",
        "indirect_magic" => "indirectMagic",
        "dragon_breath" => "dragonBreath",
        "player_attack" => "player",
        "mob_attack" => "mob",
        name => name,
    };
    let mut arguments = vec![crate::command_execution::entity_name(state, victim)];
    let mut key = format!("death.attack.{message_id}");
    if let Some(attacker) = attacker {
        arguments.push(crate::command_execution::entity_name(state, attacker));
        let weapon = state
            .players
            .get(&attacker)
            .map(|player| &player.inventory.slots[usize::from(player.inventory.selected.min(8))])
            .or_else(|| {
                state
                    .entities
                    .slot_of_any(attacker)
                    .and_then(|slot| state.entities.row(slot).appearance.equipment[0].as_ref())
            });
        if let Some(name) = weapon
            .filter(|stack| !stack.is_empty())
            .and_then(|stack| stack.custom_name.as_deref())
        {
            key.push_str(".item");
            arguments.push(crate::scoreboard::component(name));
        }
    } else if let Some(attacker) = state
        .entities
        .slot_of_any(victim)
        .and_then(|slot| state.entities.row(slot).loot.player_credit.as_ref())
        .filter(|credit| state.tick.saturating_sub(credit.tick) < 100)
        .map(|credit| EntityId(credit.id))
    {
        key.push_str(".player");
        arguments.push(crate::command_execution::entity_name(state, attacker));
    }
    Tag::Compound(vec![
        ("translate".into(), Tag::String(key)),
        (
            "with".into(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: arguments,
            }),
        ),
    ])
}

pub(super) fn broadcast(state: &mut TickState, victim: EntityId, content: Tag) {
    if !crate::command_rules::boolean(state, crate::command_rules::BoolRule::SHOW_DEATH_MESSAGES) {
        return;
    }
    let victim_name = state.connections.name_of(victim).unwrap_or("");
    let recipients: Vec<_> = state
        .connections
        .all_connections()
        .filter(|connection| {
            state
                .connections
                .entity_of(*connection)
                .and_then(|entity| state.connections.name_of(entity))
                .is_some_and(|name| state.scoreboard.permits_death_message(victim_name, name))
        })
        .collect();
    if state.dimension_globals.active {
        let members = state
            .dimension_globals
            .roster
            .iter()
            .filter(|player| {
                state
                    .scoreboard
                    .permits_death_message(victim_name, &player.name)
            })
            .map(|player| player.name.clone())
            .collect();
        state
            .dimension_globals
            .broadcasts
            .push(crate::dimensions::globals::Broadcast::TeamChat {
                members,
                content: content.clone(),
            });
    }
    for connection in recipients {
        state.push_player(
            connection,
            PlayerChange::StyledChat {
                content: content.clone(),
            },
        );
    }
}
