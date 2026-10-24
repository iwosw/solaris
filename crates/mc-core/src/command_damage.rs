//! Vanilla `/damage` syntax feeding the existing native damage merge.
use crate::boundary::{ChangeSet, ConnectionId, EntityId};
use crate::commands::{CommandError, Coord, SelectorKind, Target};
use crate::entities::EntityKind;
use crate::intent::DamageSource;
use crate::survival::DamageKind;
use crate::tick::{DamagePiece, TickState};

#[derive(Debug, Clone, PartialEq)]
pub struct DamageCommand {
    pub(crate) target: Target,
    amount: f32,
    kind: DamageKind,
    damage_type: &'static str,
    source: Source,
}

#[derive(Debug, Clone, PartialEq)]
enum Source {
    None,
    At([Coord; 3]),
    By(Box<Attribution>),
}

#[derive(Debug, Clone, PartialEq)]
struct Attribution {
    direct: Target,
    cause: Option<Target>,
}

fn single_target(word: &str) -> Result<Target, CommandError> {
    let target = crate::commands::target_of(word)?;
    let multiple = match &target {
        Target::AllPlayers => true,
        Target::Selector(selector) => {
            selector.limit.unwrap_or(match selector.kind {
                SelectorKind::Nearest | SelectorKind::Random | SelectorKind::Executor => 1,
                SelectorKind::AllPlayers | SelectorKind::Entities => usize::MAX,
            }) > 1
        }
        _ => false,
    };
    if multiple {
        return Err(CommandError::Message(
            "Damage requires exactly one entity; use limit=1".into(),
        ));
    }
    Ok(target)
}

pub(crate) fn parse(args: &[&str]) -> Result<DamageCommand, CommandError> {
    const USAGE: &str =
        "damage <target> <amount> [<damageType>] [at <location>|by <entity> [from <cause>]]";
    let [target, amount, rest @ ..] = args else {
        return Err(CommandError::Usage(USAGE));
    };
    let amount: f32 = amount.parse().map_err(|_| CommandError::Usage(USAGE))?;
    if !amount.is_finite() || amount < 0.0 {
        return Err(CommandError::Message(
            "Damage amount must be finite and nonnegative".into(),
        ));
    }
    let (name, rest) = rest
        .split_first()
        .map_or(("minecraft:generic", &[][..]), |(name, rest)| (*name, rest));
    let name = mc_data::Identifier::parse(name)
        .map_err(|_| CommandError::Message(format!("Invalid damage type '{name}'")))?;
    let (kind, damage_type) = DamageKind::command_type(name.as_str())
        .ok_or_else(|| CommandError::Message(format!("Unsupported damage type '{name}'")))?;
    let source = match rest {
        [] => Source::None,
        ["at", x, y, z] => {
            Source::At(crate::commands::coords(&[x, y, z]).ok_or(CommandError::Usage(USAGE))?)
        }
        ["by", direct] => Source::By(Box::new(Attribution {
            direct: single_target(direct)?,
            cause: None,
        })),
        ["by", direct, "from", cause] => Source::By(Box::new(Attribution {
            direct: single_target(direct)?,
            cause: Some(single_target(cause)?),
        })),
        _ => return Err(CommandError::Usage(USAGE)),
    };
    Ok(DamageCommand {
        target: single_target(target)?,
        amount,
        kind,
        damage_type,
        source,
    })
}

fn resolve(
    state: &TickState,
    executor: EntityId,
    target: &Target,
) -> Result<EntityId, CommandError> {
    let selected = crate::command_execution::resolve_target(state, executor, target);
    match selected.as_slice() {
        [entity] => Ok(*entity),
        _ => Err(CommandError::Message(
            "Damage requires exactly one matching entity".into(),
        )),
    }
}

fn descriptor(state: &TickState, entity: EntityId) -> (EntityId, u32) {
    let slot = state
        .entities
        .slot_of(entity)
        .expect("resolved damage entity");
    let type_id = match state.entities.body(slot).kind {
        EntityKind::Player { .. } => {
            mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:player")
                .expect("player contract")
                .protocol_id
        }
        EntityKind::Mob { type_id }
        | EntityKind::Item { type_id }
        | EntityKind::ExperienceOrb { type_id } => type_id,
    };
    (entity, type_id)
}

pub(crate) fn execute(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    command: &DamageCommand,
    changes: &mut ChangeSet,
) -> Result<(), CommandError> {
    let target = resolve(state, executor, &command.target)?;
    let (direct, cause, position) = match &command.source {
        Source::None => (None, None, None),
        Source::At(coords) => {
            let (origin, rotation) =
                crate::command_console::pose(state, executor).ok_or_else(|| {
                    CommandError::Message("Damage source position has no command origin".into())
                })?;
            (
                None,
                None,
                Some(crate::commands::resolve_coords(*coords, origin, rotation)),
            )
        }
        Source::By(attribution) => {
            let direct = resolve(state, executor, &attribution.direct)?;
            let cause = attribution
                .cause
                .as_ref()
                .map_or(Ok(direct), |cause| resolve(state, executor, cause))?;
            let slot = state
                .entities
                .slot_of(direct)
                .expect("resolved damage source");
            (
                Some(descriptor(state, direct)),
                Some(descriptor(state, cause)),
                Some(state.entities.pos(slot)),
            )
        }
    };
    let slot = state
        .entities
        .slot_of(target)
        .expect("resolved damage target");
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount: command.amount,
            kind: command.kind,
            source: Some(DamageSource::Command {
                connection,
                damage_type: command.damage_type,
                direct,
                cause,
                position,
            }),
        },
    ));
    // Commit at the ordinary command boundary; the native hit admission reports
    // the actual outcome, including invulnerability, shields and hurt cooldown.
    crate::dimensions::commands::commit(state, changes);
    Ok(())
}

pub(crate) fn report(
    state: &mut TickState,
    target: Option<EntityId>,
    piece: &DamagePiece,
    amount: f32,
    accepted: bool,
) {
    let Some(DamageSource::Command { connection, .. }) = piece.source else {
        return;
    };
    if accepted && let Some(target) = target {
        crate::command_execution::feedback(
            state,
            connection,
            "commands.damage.success",
            vec![
                mc_nbt::Tag::Compound(vec![(
                    "text".into(),
                    mc_nbt::Tag::String(amount.to_string()),
                )]),
                crate::command_execution::entity_name(state, target),
            ],
        );
    } else {
        crate::command_execution::feedback(
            state,
            connection,
            "commands.damage.invulnerable",
            Vec::new(),
        );
    }
}

#[cfg(test)]
#[path = "command_damage_tests.rs"]
mod tests;
