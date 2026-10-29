//! Special recipe copying a banner's decoration onto an undecorated shield.
use crate::{
    ItemStack, item_components::ItemFactsTable, item_stack::PreservedItemComponent,
    items::ItemRegistry,
};

#[must_use]
pub fn result(
    items: &ItemRegistry,
    facts: &ItemFactsTable,
    input: &[ItemStack; 9],
) -> Option<ItemStack> {
    let mut banner = None;
    let mut shield = None;
    let mut color = None;
    for stack in input.iter().filter(|stack| !stack.is_empty()) {
        if stack
            .item_model
            .as_deref()
            .is_some_and(|model| facts.custom(model).is_some())
        {
            return None;
        }
        let name = items.name_of(stack.item_id)?;
        if name.as_str() == "minecraft:shield" {
            if shield.replace(stack).is_some()
                || !stack.extended().banner_patterns.is_empty()
                || stack
                    .extended()
                    .preserved_components
                    .iter()
                    .any(|component| component.name == "minecraft:base_color")
            {
                return None;
            }
        } else if name.namespace() == "minecraft" {
            let dye = name.path().strip_suffix("_banner")?;
            if !crate::stations::DYE_COLORS.contains(&dye) || banner.replace(stack).is_some() {
                return None;
            }
            color = Some(dye);
        } else {
            return None;
        }
    }
    let banner = banner?;
    let mut output = shield?.clone();
    output.count = 1;
    let mut value = Vec::new();
    mc_nbt::write_network_value(&mut value, &mc_nbt::Tag::String(color?.into())).ok()?;
    let components = output.extended_mut();
    components
        .banner_patterns
        .clone_from(&banner.extended().banner_patterns);
    components.preserved_components.retain(|component| {
        !matches!(
            component.name.as_str(),
            "!minecraft:base_color" | "minecraft:banner_patterns" | "!minecraft:banner_patterns"
        )
    });
    components
        .preserved_components
        .push(PreservedItemComponent {
            name: "minecraft:base_color".into(),
            value,
            baseline: None,
        });
    Some(output)
}
