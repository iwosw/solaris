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

/// Iron golems drop 3-5 iron ingots and 0-2 poppies with no kill-credit condition.
#[test]
fn iron_golem_drops_ingots_without_a_player_kill() {
    let registry = CoreRegistry::load(None).expect("registry loads");
    let golem = Identifier::parse("minecraft:iron_golem").expect("id");
    let table = Identifier::parse("minecraft:entities/iron_golem").expect("id");
    let type_id = registry.entity_types.id_of(&golem).expect("golem type");
    assert_eq!(
        registry
            .blueprint(type_id)
            .and_then(|bp| bp.loot_table.clone()),
        Some(table.clone()),
        "the simulated golem blueprint must carry the vanilla loot table"
    );
    for seed in 0..32u64 {
        let drops = mob_drops(
            &registry,
            EntityLootEntity::new(golem.clone()),
            &table,
            [0.0; 3],
            seed,
            None,
            EntityDeathCause::new(Default::default(), EntityLootAttack::None, None),
        );
        let ingots: i32 = drops
            .iter()
            .filter(|stack| {
                registry
                    .items
                    .name_of(stack.item_id)
                    .map(|name| name.as_str())
                    == Some("minecraft:iron_ingot")
            })
            .map(|stack| stack.count)
            .sum();
        assert!((3..=5).contains(&ingots), "seed {seed}: {ingots} ingots");
    }
}
