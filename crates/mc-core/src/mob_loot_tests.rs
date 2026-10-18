use mc_data::Identifier;
use mc_data::loot::entity_26_1_2::{EntityDeathCause, EntityLootAttack, EntityLootEntity};

use super::mob_drops;
use crate::registry::CoreRegistry;

fn wool_drops(wool: u8) -> Vec<String> {
    let registry = CoreRegistry::load(None).expect("registry loads");
    let sheep = Identifier::parse("minecraft:sheep").expect("id");
    let table = Identifier::parse("minecraft:entities/sheep").expect("id");
    mob_drops(
        &registry,
        EntityLootEntity::new(sheep),
        &table,
        [0.0; 3],
        7,
        Some(wool),
        EntityDeathCause::new(Default::default(), EntityLootAttack::None, None),
    )
    .iter()
    .filter_map(|stack| registry.items.name_of(stack.item_id))
    .map(|name| name.as_str().to_owned())
    .filter(|name| name.ends_with("_wool"))
    .collect()
}

/// A sheep drops one wool of its own color; a sheared one drops none.
#[test]
fn sheep_drop_wool_of_their_color_unless_sheared() {
    assert_eq!(wool_drops(15), vec!["minecraft:black_wool".to_owned()]);
    assert_eq!(wool_drops(0), vec!["minecraft:white_wool".to_owned()]);
    assert!(wool_drops(0x10 | 15).is_empty());
}
