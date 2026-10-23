//! Carrier-backed custom blocks (ADR 0018).
//!
//! A plugin Loader bundle declares one block id, the reserved vanilla carrier
//! block that stores it, and the custom item it drops. The world, WAL and
//! native store only ever see the carrier states, so persistence is unchanged
//! and vanilla clients show the carrier. Placement is the custom item's own
//! vanilla carrier-item placement (facing included); breaking drops the custom
//! item. Real carrier items cannot be placed, so a carrier state always means
//! the custom block.

use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_data::{Identifier, ItemStack};
use mc_loader::CarrierBlockDeclaration;
use mc_world::BlockRegistry;

use crate::config::CoreError;

/// Vanilla blocks a plugin may reserve as a carrier: full cubes with exactly a
/// horizontal `facing` property, no block entity, never worldgen-placed.
const RESERVED_CARRIERS: [&str; 1] = ["minecraft:black_glazed_terracotta"];
const HORIZONTAL_FACINGS: [&str; 4] = ["east", "north", "south", "west"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CarrierBlock {
    pub id: Identifier,
    pub name: String,
    pub carrier: Identifier,
    pub drop_item: Identifier,
    states: Vec<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CarrierBlocks {
    blocks: Vec<CarrierBlock>,
}

impl CarrierBlocks {
    /// Validate the declarations against the block registry and the installed
    /// custom items. Call after custom items are installed.
    pub(crate) fn admit(
        declarations: &[CarrierBlockDeclaration],
        blocks: &BlockRegistry,
        facts: &ItemFactsTable,
    ) -> Result<Self, CoreError> {
        let mut admitted: Vec<CarrierBlock> = Vec::with_capacity(declarations.len());
        for declaration in declarations {
            let fail = |reason: &str| {
                CoreError::Registry(format!(
                    "carrier block {} is unsupported: {reason}",
                    declaration.id
                ))
            };
            let id = Identifier::parse(declaration.id.clone()).map_err(|e| fail(&e.to_string()))?;
            let carrier =
                Identifier::parse(declaration.carrier.clone()).map_err(|e| fail(&e.to_string()))?;
            let drop_item = Identifier::parse(declaration.drop_item.clone())
                .map_err(|e| fail(&e.to_string()))?;
            if !RESERVED_CARRIERS.contains(&carrier.as_str()) {
                return Err(fail("carrier is not a reserved carrier block"));
            }
            let block = blocks
                .block(&carrier)
                .ok_or_else(|| fail("carrier block is not registered"))?;
            let facings_ok = matches!(
                block.properties.as_slice(),
                [(name, values)] if name == "facing" && {
                    let mut sorted: Vec<&str> = values.iter().map(String::as_str).collect();
                    sorted.sort_unstable();
                    sorted == HORIZONTAL_FACINGS
                }
            );
            if !facings_ok {
                return Err(fail(
                    "carrier must have exactly a horizontal facing property",
                ));
            }
            let item = facts
                .custom(&drop_item)
                .ok_or_else(|| fail("drop item is not an installed custom item"))?;
            if item.carrier != carrier {
                return Err(fail("drop item carrier must be the carrier block's item"));
            }
            if admitted
                .iter()
                .any(|other| other.id == id || other.carrier == carrier)
            {
                return Err(fail("identity or carrier already declared"));
            }
            admitted.push(CarrierBlock {
                id,
                name: declaration.name.clone(),
                carrier,
                drop_item,
                states: block.states.iter().map(|state| state.0).collect(),
            });
        }
        Ok(Self { blocks: admitted })
    }

    pub(crate) fn for_state(&self, state: u32) -> Option<&CarrierBlock> {
        self.blocks
            .iter()
            .find(|block| block.states.contains(&state))
    }

    /// Whether `block` stores a custom block and so may only be placed by its own item.
    pub(crate) fn reserves(&self, block: &Identifier) -> bool {
        self.blocks
            .iter()
            .any(|reserved| reserved.carrier == *block)
    }

    /// The custom item a broken carrier state drops.
    pub(crate) fn drop_for_state(
        &self,
        state: u32,
        facts: &ItemFactsTable,
        items: &ItemRegistry,
    ) -> Option<ItemStack> {
        custom_stack(facts, items, &self.for_state(state)?.drop_item)
    }

    /// Whether `item` is the custom item of a carrier block.
    pub(crate) fn is_block_item(&self, item: &Identifier) -> bool {
        self.blocks.iter().any(|block| block.drop_item == *item)
    }

    /// Stable identity text for the world identity.
    pub(crate) fn identity(&self) -> Vec<String> {
        let mut identity = self
            .blocks
            .iter()
            .map(|block| {
                format!(
                    "{}|{}|{}|{}",
                    block.id, block.name, block.carrier, block.drop_item
                )
            })
            .collect::<Vec<_>>();
        identity.sort_unstable();
        identity
    }
}

/// One stack of an installed custom item on its vanilla carrier.
pub(crate) fn custom_stack(
    facts: &ItemFactsTable,
    items: &ItemRegistry,
    id: &Identifier,
) -> Option<ItemStack> {
    let custom = facts.custom(id)?;
    Some(
        ItemStack::new(items.id_of(&custom.carrier)?, 1)
            .with_item_model(custom.id.clone())
            .with_custom_name(&custom.name),
    )
}
