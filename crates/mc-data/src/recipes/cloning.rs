//! Dynamic copy recipes: the source remains in the grid, only blanks are spent.
use crate::{ItemStack, item_components::ItemFactsTable, items::ItemRegistry, tags::TagsData};

const BOOK_CONTENT: &str = "minecraft:written_book_content";

fn book_content(stack: &ItemStack) -> Option<(mc_nbt::Tag, i32)> {
    let component = stack
        .extended()
        .preserved_components
        .iter()
        .find(|c| c.name == BOOK_CONTENT)?;
    let mut bytes = component.value.as_slice();
    let tag = mc_nbt::read_network(&mut bytes).ok()?;
    if !bytes.is_empty() {
        return None;
    }
    let mc_nbt::Tag::Compound(fields) = &tag else {
        return None;
    };
    let generation = match fields.iter().find(|(key, _)| key == "generation") {
        None => 0,
        Some((_, mc_nbt::Tag::Int(value))) => *value,
        _ => return None,
    };
    (0..2).contains(&generation).then_some((tag, generation))
}

/// Source index for a valid book/banner copying grid. At most nine cells.
pub fn source(
    items: &ItemRegistry,
    facts: &ItemFactsTable,
    tags: &TagsData,
    input: &[ItemStack],
) -> Option<usize> {
    let mut source = None;
    let mut blank = None;
    for (index, stack) in input.iter().enumerate().filter(|(_, s)| !s.is_empty()) {
        if stack
            .item_model
            .as_deref()
            .is_some_and(|model| facts.custom(model).is_some())
        {
            return None;
        }
        let name = items.name_of(stack.item_id)?;
        let patterned = name.namespace() == "minecraft"
            && name.path().ends_with("_banner")
            && !stack.extended().banner_patterns.is_empty();
        if patterned || name.as_str() == "minecraft:written_book" {
            if source.replace(index).is_some() {
                return None;
            }
        } else {
            blank = Some(stack.item_id);
        }
    }
    let index = source?;
    let original = &input[index];
    let blank = blank?;
    let name = items.name_of(original.item_id)?;
    if name.as_str() == "minecraft:written_book" {
        book_content(original)?;
        for (i, stack) in input.iter().enumerate().filter(|(_, s)| !s.is_empty()) {
            if i != index
                && !tags.contains_raw_id(
                    "minecraft:item",
                    "minecraft:book_cloning_target",
                    i32::try_from(stack.item_id).ok()?,
                )
            {
                return None;
            }
        }
    } else if original.extended().banner_patterns.len() > 6
        || original.item_id != blank
        || input.iter().filter(|s| !s.is_empty()).count() != 2
    {
        return None;
    }
    Some(index)
}

pub fn result(
    items: &ItemRegistry,
    facts: &ItemFactsTable,
    tags: &TagsData,
    input: &[ItemStack; 9],
) -> Option<ItemStack> {
    let index = source(items, facts, tags, input)?;
    let mut result = input[index].clone();
    result.count = (input.iter().filter(|s| !s.is_empty()).count() - 1) as i32;
    if items.name_of(result.item_id)?.as_str() == "minecraft:written_book" {
        let (mut tag, generation) = book_content(&result)?;
        let mc_nbt::Tag::Compound(fields) = &mut tag else {
            return None;
        };
        if let Some((_, value)) = fields.iter_mut().find(|(key, _)| key == "generation") {
            *value = mc_nbt::Tag::Int(generation + 1);
        } else {
            fields.push(("generation".into(), mc_nbt::Tag::Int(generation + 1)));
        }
        let content = result
            .extended_mut()
            .preserved_components
            .iter_mut()
            .find(|c| c.name == BOOK_CONTENT)?;
        content.value.clear();
        mc_nbt::write_network(&mut content.value, &tag).ok()?;
        content.baseline = None;
    }
    Some(result)
}
