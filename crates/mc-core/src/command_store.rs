//! Execute result callbacks over outcomes produced by native command owners.
use crate::boundary::EntityId;
use crate::command_context::Context;
use crate::tick::TickState;

#[derive(Clone)]
pub(crate) struct Store {
    holders: std::sync::Arc<[String]>,
    objective: String,
    success: bool,
}
fn word<'a>(input: &mut &'a str) -> Result<&'a str, String> {
    crate::command_nbt_path::argument(input)
}
fn holder(state: &TickState, id: EntityId) -> Result<String, String> {
    let (dimension, slot) =
        crate::dimensions::travel::locate(state, id).ok_or("No entity was found")?;
    let world = crate::dimensions::travel::world(state, dimension).ok_or("Unknown dimension")?;
    Ok(world
        .connections
        .name_of(id)
        .map(str::to_owned)
        .unwrap_or_else(|| world.entities.row(slot).uuid.to_string()))
}
fn holders(state: &TickState, context: Context, name: &str) -> Result<Vec<String>, String> {
    let result = if name == "*" {
        state.scoreboard.scores.keys().cloned().collect()
    } else if name.starts_with('@') {
        let target = crate::commands::target_of(name).map_err(|e| e.message())?;
        crate::dimensions::travel::context_targets(state, context, &target)?
            .into_iter()
            .map(|id| holder(state, id))
            .collect::<Result<_, _>>()?
    } else {
        vec![name.to_owned()]
    };
    if result.is_empty() {
        return Err("No entities were found".into());
    }
    Ok(result)
}
impl Store {
    pub(crate) fn parse(
        state: &TickState,
        context: Context,
        input: &mut &str,
    ) -> Result<Self, String> {
        let success = match word(input)? {
            "success" => true,
            "result" => false,
            _ => return Err("Expected result or success".into()),
        };
        if word(input)? != "score" {
            return Err("Only execute store score is supported".into());
        }
        let holders = holders(state, context, word(input)?)?.into();
        let objective = word(input)?;
        if !state.scoreboard.objectives.contains_key(objective) {
            return Err(format!("Unknown scoreboard objective '{objective}'"));
        }
        Ok(Self {
            holders,
            objective: objective.to_owned(),
            success,
        })
    }
    pub(crate) fn apply(&self, state: &mut TickState, success: bool, value: i32) {
        crate::scoreboard::store_result(
            state,
            &self.holders,
            &self.objective,
            if self.success {
                i32::from(success)
            } else {
                value
            },
        );
    }
}
