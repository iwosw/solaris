//! Exact gross recipe portions, with native causal deadlines and durable ACKs.
use super::*;
use mc_data::recipes::work::{
    WorkRecipe, WorkRecipeAlternative, WorkRecipeComponents, WorkRecipeInput, WorkRecipeStack,
};
use mc_data::recipes::{IngredientAlternative, RecipeKind};

pub(super) fn recipe(state: &TickState, name: &str) -> Result<WorkRecipe, ScriptWorkPauseReason> {
    let id = Identifier::parse(name).map_err(|_| ScriptWorkPauseReason::MissingInput)?;
    if let Some(recipe) = state.registry.item_facts.work_recipe(&id) {
        return Ok(recipe.clone());
    }
    let recipe = state
        .registry
        .recipes
        .iter()
        .find(|recipe| recipe.id == id)
        .ok_or(ScriptWorkPauseReason::MissingInput)?;
    let (ingredients, station, duration) = match &recipe.kind {
        RecipeKind::Shaped(shaped) => {
            let mut ingredients = Vec::new();
            for row in &shaped.pattern {
                for ch in row.chars().filter(|ch| *ch != ' ') {
                    ingredients.push(
                        shaped
                            .key
                            .get(&ch)
                            .ok_or(ScriptWorkPauseReason::MissingInput)?
                            .clone(),
                    );
                }
            }
            (ingredients, "minecraft:crafting_table", 20)
        }
        RecipeKind::Shapeless(shapeless) => (
            shapeless.ingredients.clone(),
            "minecraft:crafting_table",
            20,
        ),
        RecipeKind::Stonecutting(stonecutting) => (
            vec![stonecutting.ingredient.clone()],
            "minecraft:stonecutter",
            20,
        ),
        RecipeKind::CampfireCooking(cooking) => (
            vec![cooking.ingredient.clone()],
            "minecraft:campfire",
            u64::from(cooking.cooking_time),
        ),
        // These live burn/fuel processes are owned by native furnace custody.
        RecipeKind::Smelting(_) | RecipeKind::Smoking(_) | RecipeKind::Blasting(_) => {
            return Err(ScriptWorkPauseReason::MissingStation);
        }
    };
    let mut inputs = Vec::new();
    for ingredient in ingredients {
        inputs.push(WorkRecipeInput {
            item: None,
            tag: None,
            alternatives: ingredient
                .alternatives
                .into_iter()
                .map(|alternative| match alternative {
                    IngredientAlternative::Item(id) => WorkRecipeAlternative::Item(id),
                    IngredientAlternative::Tag(id) => WorkRecipeAlternative::Tag(id),
                })
                .collect(),
            count: 1,
            components: None,
        });
    }
    let output = recipe
        .result
        .to_stack(&state.registry.items, &state.registry.item_facts)
        .ok_or(ScriptWorkPauseReason::Unsupported)?;
    Ok(WorkRecipe {
        id: id.clone(),
        revision: inventories::hash(&format!("{recipe:?}")),
        station: Identifier::parse(station).expect("native station identifier"),
        duration_ticks: duration,
        inputs,
        outputs: vec![WorkRecipeStack {
            item: recipe.result.item.clone(),
            count: recipe.result.count,
            components: WorkRecipeComponents::of(&output),
        }],
        remainders: Vec::new(),
        tools: Vec::new(),
    })
}

pub(super) fn station(
    state: &TickState,
    area: &ScriptWorkArea,
    recipe: &WorkRecipe,
) -> Result<(), ScriptWorkPauseReason> {
    if area.dimension != state.dimension.name() {
        return Err(ScriptWorkPauseReason::Unloaded);
    }
    let native = state
        .world
        .block_at(area.min.x, area.min.y, area.min.z)
        .ok_or(ScriptWorkPauseReason::Unloaded)?;
    let block = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(native))
        .ok_or(ScriptWorkPauseReason::MissingStation)?;
    if block.block.id != recipe.station
        || (recipe.station.path() == "campfire"
            && block
                .properties
                .iter()
                .any(|(key, value)| key == "lit" && value == "false"))
    {
        return Err(ScriptWorkPauseReason::MissingStation);
    }
    Ok(())
}
struct Plan {
    carry: InventoryImage,
    equipment: InventoryImage,
    inputs: Vec<ScriptCraftStack>,
    outputs: Vec<ScriptCraftStack>,
    wear: Vec<ScriptCraftToolWear>,
    changes: Vec<ScriptItemChange>,
}
fn accepts(state: &TickState, stack: &ItemStack, input: &WorkRecipeInput) -> bool {
    if stack.is_empty()
        || input
            .components
            .as_ref()
            .is_some_and(|components| !components.matches(stack))
    {
        return false;
    }
    let identity = resource(state, stack);
    let item = input
        .item
        .as_ref()
        .is_some_and(|name| identity.as_deref() == Some(name.as_str()));
    let tag = input.tag.as_ref().is_some_and(|name| {
        state
            .registry
            .tags
            .contains_raw_id("minecraft:item", name.as_str(), stack.item_id as i32)
    });
    item || tag
        || input
            .alternatives
            .iter()
            .any(|alternative| match alternative {
                WorkRecipeAlternative::Item(name) => identity.as_deref() == Some(name.as_str()),
                WorkRecipeAlternative::Tag(name) => state.registry.tags.contains_raw_id(
                    "minecraft:item",
                    name.as_str(),
                    stack.item_id as i32,
                ),
            })
}
pub(super) fn plan(
    state: &TickState,
    id: &str,
    handle: &str,
    recipe: &WorkRecipe,
) -> Result<(), ScriptWorkPauseReason> {
    make_plan(state, id, handle, recipe).map(|_| ())
}
fn make_plan(
    state: &TickState,
    id: &str,
    handle: &str,
    recipe: &WorkRecipe,
) -> Result<Plan, ScriptWorkPauseReason> {
    let carry_endpoint = carry(handle);
    let equipment_endpoint = equipment(handle);
    let mut carry_image = inventories::load(state, id, &carry_endpoint).map_err(reason)?;
    let mut equipment_image = inventories::load(state, id, &equipment_endpoint).map_err(reason)?;
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    let mut changes = Vec::new();
    let mut wear_changes = Vec::new();
    for input in &recipe.inputs {
        let mut remaining = input.count;
        for (endpoint, image) in [
            (&carry_endpoint, &mut carry_image),
            (&equipment_endpoint, &mut equipment_image),
        ] {
            for (&slot, stack) in &mut image.slots {
                if remaining == 0 {
                    break;
                }
                if !accepts(state, stack, input) {
                    continue;
                }
                let count = remaining.min(stack.count.max(0) as u32);
                let mut taken = stack.clone();
                taken.count = count as i32;
                inputs.push(ScriptCraftStack::new(
                    endpoint.clone(),
                    slot,
                    inventories::to_script_item(state, &taken).map_err(reason)?,
                ));
                if let Some(name) = resource(state, stack) {
                    changes.push(ScriptItemChange::new(name, -i64::from(count)));
                }
                stack.count -= count as i32;
                remaining -= count;
                if stack.is_empty() {
                    *stack = ItemStack::EMPTY;
                }
            }
        }
        if remaining != 0 {
            return Err(ScriptWorkPauseReason::MissingInput);
        }
    }
    // Native use-remainder components return their exact container item.
    let mut native_outputs = Vec::new();
    if state.registry.item_facts.work_recipe(&recipe.id).is_none() {
        for input in &inputs {
            let input_id = Identifier::parse(&input.item.resource_id)
                .map_err(|_| ScriptWorkPauseReason::Unsupported)?;
            if let Some(remainder) = state
                .registry
                .item_facts
                .get(&input_id)
                .and_then(|facts| facts.use_remainder.as_ref())
            {
                let stack = mc_data::recipes::RecipeResult {
                    item: remainder.clone(),
                    count: input.item.count,
                    stew_effects: Vec::new(),
                }
                .to_stack(&state.registry.items, &state.registry.item_facts)
                .ok_or(ScriptWorkPauseReason::Unsupported)?;
                native_outputs.push(stack);
            }
        }
    }
    for tool in &recipe.tools {
        let mut found = None;
        for (endpoint, image) in [
            (&equipment_endpoint, &mut equipment_image),
            (&carry_endpoint, &mut carry_image),
        ] {
            let slot = image
                .slots
                .iter()
                .find(|(_, stack)| {
                    !stack.is_empty()
                        && resource(state, stack).as_deref() == Some(tool.item.as_str())
                        && tool
                            .components
                            .as_ref()
                            .is_none_or(|components| components.matches(stack))
                })
                .map(|(&slot, _)| slot);
            if let Some(slot) = slot {
                let before = image.slots.get(&slot).expect("native tool slot").clone();
                if tool.wear > 0
                    && super::maximum_damage(state, &before, tool.item.as_str()).is_none()
                {
                    return Err(ScriptWorkPauseReason::MissingTool);
                }
                let before = ScriptCraftStack::new(
                    endpoint.clone(),
                    slot,
                    inventories::to_script_item(state, &before).map_err(reason)?,
                );
                if let Some(change) = super::wear(state, image, slot, tool.wear) {
                    changes.push(change);
                }
                let after = image
                    .slots
                    .get(&slot)
                    .filter(|stack| !stack.is_empty())
                    .map(|stack| inventories::to_script_item(state, stack))
                    .transpose()
                    .map_err(reason)?;
                if tool.wear > 0 {
                    wear_changes.push(ScriptCraftToolWear::new(before, after, tool.wear));
                }
                found = Some(());
                break;
            }
        }
        if found.is_none() {
            return Err(ScriptWorkPauseReason::MissingTool);
        }
    }
    for output in recipe.outputs.iter().chain(&recipe.remainders) {
        native_outputs.push(
            output
                .to_stack(&state.registry.items, &state.registry.item_facts)
                .ok_or(ScriptWorkPauseReason::Unsupported)?,
        );
    }
    for stack in native_outputs {
        let before = carry_image.slots.clone();
        inventories::insert(state, &mut carry_image, stack.clone()).map_err(reason)?;
        let name = resource(state, &stack).ok_or(ScriptWorkPauseReason::Unsupported)?;
        changes.push(ScriptItemChange::new(name, i64::from(stack.count)));
        for (&slot, after) in &carry_image.slots {
            let old = before.get(&slot).map_or(0, |stack| stack.count);
            let delta = after.count - old;
            if delta > 0 {
                let mut exact = after.clone();
                exact.count = delta;
                outputs.push(ScriptCraftStack::new(
                    carry_endpoint.clone(),
                    slot,
                    inventories::to_script_item(state, &exact).map_err(reason)?,
                ));
            }
        }
    }
    Ok(Plan {
        carry: carry_image,
        equipment: equipment_image,
        inputs,
        outputs,
        wear: wear_changes,
        changes,
    })
}

pub(super) fn perform(
    state: &mut TickState,
    id: &str,
    (record, slot): (&NpcRecord, EntitySlot),
    assignment: &mut ScriptWorkAssignment,
    name: &str,
    station_area: &ScriptWorkArea,
    revision: u64,
) -> Result<Vec<ScriptItemChange>, ScriptWorkPauseReason> {
    let recipe = recipe(state, name)?;
    station(state, station_area, &recipe)?;
    near(state, slot, station_area.min)?;
    let plan = make_plan(state, id, &record.handle, &recipe)?;
    let carry_endpoint = carry(&record.handle);
    let equipment_endpoint = equipment(&record.handle);
    let before = vec![
        ScriptInventoryExpectedRevision::new(carry_endpoint.clone(), plan.carry.fence.clone()),
        ScriptInventoryExpectedRevision::new(
            equipment_endpoint.clone(),
            plan.equipment.fence.clone(),
        ),
    ];
    inventories::commit(
        state,
        id,
        BTreeMap::from([
            (carry_endpoint.clone(), plan.carry),
            (equipment_endpoint.clone(), plan.equipment),
        ]),
    )
    .map_err(reason)?;
    let after = vec![
        ScriptInventoryExpectedRevision::new(
            carry_endpoint.clone(),
            inventories::load(state, id, &carry_endpoint)
                .map_err(reason)?
                .fence,
        ),
        ScriptInventoryExpectedRevision::new(
            equipment_endpoint.clone(),
            inventories::load(state, id, &equipment_endpoint)
                .map_err(reason)?
                .fence,
        ),
    ];
    let operation_id = assignment
        .operation_id
        .clone()
        .expect("admitted native work identity");
    let sequence = assignment.work_units_done + 1;
    let portion_id = inventories::hash(&(&operation_id, &record.handle, sequence));
    assignment.receipts.push(ScriptCraftReceipt::new(
        operation_id,
        record.handle.clone(),
        portion_id,
        sequence,
        recipe.id.to_string(),
        recipe.revision,
        state.tick,
        state.tick,
        recipe.duration_ticks,
        station_area.clone(),
        plan.inputs,
        plan.outputs,
        plan.wear,
        before,
        after,
        revision,
    ));
    Ok(plan.changes)
}
