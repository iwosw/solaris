//! Immutable startup data: registries, collision classes, mob blueprints.
//!
//! Built once in [`Core::open`](crate::core::Core::open) and shared by
//! reference afterwards. Everything here is read-only on the tick path.
//!
//! Unit tests clone a process-wide embedded template: expensive deterministic
//! resolution runs once, while each fixture retains its own mutable tables.

use std::sync::Arc;

use mc_data::Identifier;
use mc_data::blocks::solaris_required_blocks_report;
use mc_data::entity_types::solaris_required_entity_types;
use mc_data::items::solaris_required_items;
use mc_physics::BlockMaterialIds;
use mc_world::BlockRegistry;

use crate::blueprint::{MobBlueprint, vanilla_blueprints};
use crate::config::CoreError;

type LoadedVanillaData = (
    mc_data::item_components::ItemFactsTable,
    Arc<mc_data::tags::TagsData>,
    Vec<mc_data::recipes::Recipe>,
    mc_data::loot::LootTables,
    mc_data::block_mining::BlockMiningTable,
    Arc<mc_worldgen::vanilla_features::CacheTags>,
);

/// Dense contact-hazard flags indexed by global block-state id, built once
/// at registry load so the tick never matches names or reads properties.
pub(crate) const HAZARD_WATER: u8 = 0x01;
pub(crate) const HAZARD_LAVA: u8 = 0x02;
pub(crate) const HAZARD_FIRE: u8 = 0x04;
pub(crate) const HAZARD_CACTUS: u8 = 0x08;
pub(crate) const HAZARD_BERRY: u8 = 0x10;
pub(crate) const HAZARD_CAMPFIRE: u8 = 0x20;
pub(crate) const HAZARD_MAGMA: u8 = 0x40;

/// Everything the tick reads but never writes.
#[cfg_attr(test, derive(Clone))]
pub struct CoreRegistry {
    /// Dense global block state table.
    pub blocks: Arc<BlockRegistry>,
    /// Item name <-> protocol id table.
    pub items: std::sync::Arc<mc_data::items::ItemRegistry>,
    /// Entity name <-> protocol id table.
    pub entity_types: mc_data::entity_types::EntityTypeRegistry,
    /// Physics collision classification over global state ids.
    pub materials: BlockMaterialIds,
    /// Exact context-free collision boxes indexed densely by global state id.
    pub(crate) collision_boxes: Vec<Box<[mc_physics::BlockCollisionBox]>>,
    /// Fingerprinted selection outlines and native positional offsets.
    pub(crate) outlines: Vec<crate::pov_ray::OutlineState>,
    /// Largest box height in sixteenths, including fences and walls.
    pub(crate) max_collision_box_y: u8,
    /// Upper bound for ordinary collision candidates per sampled cell.
    pub(crate) max_collision_boxes_per_cell: usize,
    /// Resolved block states the tick needs by name.
    pub states: ResolvedStates,
    /// Mob blueprints keyed by blueprint key (protocol entity type id, or a
    /// data-defined [`CUSTOM_TYPE_FLAG`](crate::blueprint::CUSTOM_TYPE_FLAG)
    /// key). Mutate only through methods that rebuild `blueprint_index`.
    pub(crate) blueprints: Vec<(u32, MobBlueprint)>,
    /// Dense key -> position in `blueprints` (`u32::MAX` = none); vanilla
    /// keys first, data-defined ordinals after `ENTITY_TYPE_COUNT`.
    blueprint_index: Vec<u32>,
    /// Vanilla biome creature weights and herd sizes, resolved at startup.
    pub biome_spawns: mc_data::biomes::BiomeSpawnRules,
    /// Item entity blueprint: protocol type id of `minecraft:item`.
    pub item_type_id: u32,
    /// Protocol type ids of the passive breeder pair.
    pub cow_type_id: u32,
    /// Protocol type id of the hostile chaser.
    pub zombie_type_id: u32,
    /// Item id of `minecraft:shears` (shearing wool mobs).
    pub shears_item: Option<u32>,
    /// Item facts (stack sizes, food, use durations) for [`ItemRules`](crate::inventory::ItemRules).
    pub item_facts: mc_data::item_components::ItemFactsTable,
    /// Canonical vanilla tags, shared with village processors and biome gates.
    pub tags: Arc<mc_data::tags::TagsData>,
    /// View over the already-loaded registry/tag data; no worldgen reload or copy.
    pub(crate) worldgen_tags: Option<Arc<mc_worldgen::vanilla_features::CacheTags>>,
    /// Crafting/furnace recipes, index = recipe display id (contract).
    pub recipes: Vec<mc_data::recipes::Recipe>,
    pub advancements: Arc<mc_data::advancements::AdvancementTree>,
    /// Immutable advancement block properties/tags, populated once per state.
    pub(crate) advancement_blocks: Vec<std::sync::OnceLock<serde_json::Value>>,
    pub(crate) advancement_reward_loot:
        std::collections::BTreeMap<String, mc_data::loot::chest_26_1_2::ChestLootTable>,
    pub enchantments: crate::inventory::enchanting::EnchantmentCatalog,
    /// Vanilla block loot tables (block breaks).
    pub loot: mc_data::loot::LootTables,
    /// Immutable catalogs loaded from the same selected vanilla directory.
    pub entity_loot: Arc<mc_data::loot::entity_26_1_2::EntityLootCatalog>,
    pub entity_loot_lookups: Arc<mc_data::loot::entity_26_1_2::VanillaEntityLootLookups>,
    pub chest_loot: Arc<mc_data::loot::chest_26_1_2::ChestLootCatalog>,
    pub fishing_loot: Arc<mc_data::loot::fishing_26_1_2::FishingLootTables>,
    pub trial_loot: Arc<mc_data::trial_loot::TrialLootTables>,
    /// Vanilla block-mining table (correct-tool-for-drops gate).
    pub mining: mc_data::block_mining::BlockMiningTable,
    /// Farming meaning of every block state (crops, farmland, soil).
    pub farming: crate::farming::FarmStates,
    /// Dense per-state contact-hazard flags (`HAZARD_*`); index is the global
    /// state id. Water covers every level; berry only `age != 0` states and
    /// campfire only `lit == true` states, so the tick needs no properties.
    pub(crate) hazard_flags: Vec<u8>,
    /// Dense per-state wooden-door flags indexed by global state id, built
    /// once at load so the A* hot path needs no registry lookup or 12-way
    /// string match. The embedded registry lacks the `wooden_doors` tag, so
    /// the table uses the exact 12-family match in both registry modes.
    /// Shared (`Arc`) so world constructors clone without copying the table.
    pub(crate) wooden_doors: Arc<[bool]>,
    /// Native world-fire state, attachment, support and ignition tables.
    pub(crate) fire_states: crate::fire::FireStates,
    /// Per-state blast resistance (`reports/block_explosion.json`); absent
    /// without a vanilla data dir (see [`crate::explosion::blast_resistance`]).
    pub explosion: Option<mc_data::block_explosion::BlockExplosionTable>,
    /// Exact vanilla light, opacity and suffocation oracle, shared by worlds.
    pub light: Arc<mc_data::block_light::BlockLightTable>,
    /// Configure rules admitted before world loading; terrain consumes tree/clay rules.
    pub(crate) plugin_rules: Option<mc_script::GameplayRules>,
    /// Deployment-owned ore policy, applied before spawn discovery and generation.
    pub(crate) plugin_ore_profile: Option<mc_script::PluginWorldgenOreProfile>,
    /// Verified package artifacts for the network's Loader catalog.
    pub plugin_client_bundles: Vec<mc_script::ClientBundle>,
    /// Admitted authored identities; their definitions live only in `item_facts`.
    pub(crate) plugin_item_ids: Vec<Identifier>,
    /// Authored work identities, resolved through the same native facts authority.
    pub(crate) plugin_work_recipe_ids: Vec<Identifier>,
    /// Plugin-authored crafting recipes (also present in `recipes`), kept for the world identity.
    pub(crate) plugin_crafting_recipes: Vec<mc_data::recipes::Recipe>,
    /// Plugin-authored block-tool rules admitted against the block registry.
    pub(crate) block_tools: crate::block_tools::BlockToolTable,
}

impl std::fmt::Debug for CoreRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreRegistry")
            .field("recipes", &self.recipes.len())
            .finish_non_exhaustive()
    }
}

impl CoreRegistry {
    /// [`ItemRules`](crate::inventory::ItemRules) over immutable startup data.
    #[must_use]
    pub fn item_rules(&self) -> crate::inventory::ItemRules<'_> {
        crate::inventory::ItemRules {
            items: &self.items,
            facts: &self.item_facts,
            tags: &self.tags,
            recipes: &self.recipes,
        }
    }

    /// How many plugin-authored block-tool rules were admitted at startup.
    #[must_use]
    pub fn block_tool_rule_count(&self) -> usize {
        self.block_tools.rules().len()
    }

    /// Borrow all canonical custom item definitions from the admitted facts.
    pub fn plugin_items(
        &self,
    ) -> impl Iterator<Item = &mc_data::item_components::CustomItemDefinition> {
        self.plugin_item_ids.iter().map(|id| {
            self.item_facts
                .custom(id)
                .expect("admitted plugin item is installed in native facts")
        })
    }

    /// Installed identities owned by the native catalog, not a plugin namespace.
    #[must_use]
    pub fn is_native_item(&self, id: &Identifier) -> bool {
        self.item_facts.custom(id).is_some()
            && crate::settlements::startup::ITEMS
                .iter()
                .any(|(native_id, ..)| *native_id == id.as_str())
    }
}

/// Frequently used block states, resolved once at startup.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedStates {
    pub air: u32,
    pub stone: u32,
    pub dirt: u32,
    pub sand: u32,
    pub gravel: u32,
    pub water: u32,
    pub torch: u32,
    pub grass_block: u32,
    pub obsidian: u32,
    pub cobblestone: u32,
    /// `water` / `lava` by `level` (0 source, 1-7 flowing, 8-15 falling).
    pub water_levels: [u32; 16],
    pub lava_levels: [u32; 16],
}

impl CoreRegistry {
    pub(crate) fn load(vanilla_data_dir: Option<&std::path::Path>) -> Result<Self, CoreError> {
        #[cfg(test)]
        if vanilla_data_dir.is_none() {
            static EMBEDDED: std::sync::LazyLock<Result<CoreRegistry, CoreError>> =
                std::sync::LazyLock::new(|| CoreRegistry::load_uncached(None));
            return EMBEDDED.clone();
        }
        Self::load_uncached(vanilla_data_dir)
    }

    fn load_uncached(vanilla_data_dir: Option<&std::path::Path>) -> Result<Self, CoreError> {
        #[cfg(test)]
        let fixture_root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
        #[cfg(test)]
        let root = vanilla_data_dir.unwrap_or(&fixture_root);
        #[cfg(not(test))]
        let root = vanilla_data_dir.ok_or_else(|| {
            CoreError::Registry("vanilla content directory must be selected before startup".into())
        })?;
        let report = solaris_required_blocks_report();
        let blocks = Arc::new(
            BlockRegistry::from_report(&report)
                .map_err(|err| CoreError::Registry(err.to_string()))?,
        );
        let items = solaris_required_items();
        let entity_types = solaris_required_entity_types();
        let entity_loot = Arc::new(
            mc_data::loot::entity_26_1_2::load_vanilla(root).map_err(CoreError::Registry)?,
        );
        let entity_loot_lookups = Arc::new(
            mc_data::loot::entity_26_1_2::load_vanilla_lookups(root)
                .map_err(CoreError::Registry)?,
        );
        let chest_loot =
            Arc::new(mc_data::loot::chest_26_1_2::load_vanilla(root).map_err(CoreError::Registry)?);
        let fishing_loot = Arc::new(
            mc_data::loot::fishing_26_1_2::FishingLootTables::load(root)
                .map_err(CoreError::Registry)?,
        );
        let trial_loot = Arc::new(
            mc_data::trial_loot::TrialLootTables::load(root).map_err(CoreError::Registry)?,
        );
        let light = Arc::new(
            mc_data::block_light::load(root.join("reports/block_light.json"))
                .map_err(|error| CoreError::Registry(error.to_string()))?,
        );
        let (item_facts, tags, recipes, loot, mining, worldgen_tags) = match vanilla_data_dir {
            Some(dir) => {
                let (item_facts, tags, recipes, loot, mining, worldgen_tags) =
                    Self::load_full_data(dir, &items, &blocks)?;
                (item_facts, tags, recipes, loot, mining, Some(worldgen_tags))
            }
            None => (
                mc_data::item_components::solaris_required_item_facts(),
                Arc::new(mc_data::tags::solaris_required_client_tags(&items, &report)),
                mc_data::recipes::solaris_required_recipes(),
                mc_data::loot::load_builtin(root)
                    .map_err(|error| CoreError::Registry(error.to_string()))?,
                mc_data::block_mining::BlockMiningTable::default(),
                None,
            ),
        };
        let biome_spawns = match vanilla_data_dir {
            Some(dir) => mc_data::biomes::load_biome_spawn_rules(
                dir.join("data")
                    .join("minecraft")
                    .join("worldgen")
                    .join("biome"),
            )
            .map_err(|err| CoreError::Registry(err.to_string()))?,
            None => mc_data::biomes::solaris_required_biome_spawn_rules(),
        };
        let explosion = match vanilla_data_dir {
            Some(dir) => match mc_data::block_explosion::load_block_explosion_report(
                dir.join("reports").join("block_explosion.json"),
            ) {
                Ok(table) => Some(table),
                Err(mc_data::block_explosion::BlockExplosionError::Missing(_)) => None,
                Err(err) => return Err(CoreError::Registry(err.to_string())),
            },
            None => None,
        };
        let states = ResolvedStates::resolve(&blocks)?;
        let farming = crate::farming::FarmStates::resolve(&blocks)?;
        let fire_states = crate::fire::FireStates::resolve(&blocks)?;
        let water_states: Vec<u32> = blocks
            .states()
            .filter(|state| state.block.id.as_str() == "minecraft:water")
            .map(|state| state.id.0)
            .collect();
        let lava_states: Vec<u32> = blocks
            .states()
            .filter(|state| state.block.id.as_str() == "minecraft:lava")
            .map(|state| state.id.0)
            .collect();
        let shapes = mc_data::collision_shapes::vanilla_collision_shapes();
        let passable = blocks
            .states()
            .filter(|state| {
                !matches!(
                    state.block.id.as_str(),
                    "minecraft:water" | "minecraft:lava"
                )
            })
            .filter(|state| {
                shapes
                    .get_for_state(state.id.0, &state.block.id, &state.properties)
                    .is_some_and(|shape| shape.is_empty())
            })
            .map(|state| state.id.0)
            .collect();
        let materials = BlockMaterialIds::new(
            states.air,
            water_states.first().copied(),
            lava_states.first().copied(),
        )
        .with_water_states(water_states)
        .with_lava_states(lava_states)
        .with_passable(passable);
        let state_count = blocks
            .states()
            .map(|state| state.id.0 as usize + 1)
            .max()
            .unwrap_or(0);
        let mut hazard_flags = vec![0u8; state_count];
        let mut wooden_doors = vec![false; state_count];
        for state in blocks.states() {
            let name = state.block.id.as_str();
            let mut flags = 0u8;
            if name == "minecraft:water" {
                flags |= HAZARD_WATER;
            } else if name == "minecraft:lava" {
                flags |= HAZARD_LAVA;
            } else if name == "minecraft:fire" || name == "minecraft:soul_fire" {
                flags |= HAZARD_FIRE;
            } else if name == "minecraft:cactus" {
                flags |= HAZARD_CACTUS;
            } else if name == "minecraft:sweet_berry_bush"
                && state
                    .properties
                    .iter()
                    .any(|(name, value)| name == "age" && value != "0")
            {
                flags |= HAZARD_BERRY;
            } else if (name == "minecraft:campfire" || name == "minecraft:soul_campfire")
                && state
                    .properties
                    .iter()
                    .any(|(name, value)| name == "lit" && value == "true")
            {
                flags |= HAZARD_CAMPFIRE;
            } else if name == "minecraft:magma_block" {
                flags |= HAZARD_MAGMA;
            }
            hazard_flags[state.id.0 as usize] = flags;
            // Embedded tags lack `wooden_doors`; the exact 12-family match
            // covers both registry modes (iron/copper excluded).
            wooden_doors[state.id.0 as usize] = matches!(
                state.block.id.path(),
                "oak_door"
                    | "spruce_door"
                    | "birch_door"
                    | "jungle_door"
                    | "acacia_door"
                    | "dark_oak_door"
                    | "pale_oak_door"
                    | "crimson_door"
                    | "warped_door"
                    | "mangrove_door"
                    | "bamboo_door"
                    | "cherry_door"
            );
        }
        let wooden_doors: Arc<[bool]> = wooden_doors.into();
        let mut collision_boxes = vec![Box::default(); state_count];
        let outline_shapes = mc_data::outline_shapes::vanilla_outline_shapes();
        let mut outlines = vec![crate::pov_ray::OutlineState::default(); state_count];
        let mut max_collision_box_y = 16;
        // Missing/unknown states emit one conservative full-block box.
        let mut max_collision_boxes_per_cell = 1;
        for state in blocks.states() {
            let shape = outline_shapes
                .get_for_state(state.id.0, &state.block.id, &state.properties)
                .ok_or_else(|| {
                    CoreError::Registry(format!("missing vanilla outline for state {}", state.id.0))
                })?;
            let horizontal = crate::pov_ray::offset_bounds(state.block.id.as_str());
            outlines[state.id.0 as usize] = crate::pov_ray::OutlineState {
                shape: Some(shape),
                horizontal,
            };
            let boxes: Box<[_]> =
                match shapes.get_for_state(state.id.0, &state.block.id, &state.properties) {
                    Some(shape) => shape
                        .iter()
                        .map(|collision_box| {
                            mc_physics::BlockCollisionBox::from_fixed_4096(
                                collision_box.coordinates(),
                            )
                            .expect("validated vanilla collision box")
                        })
                        .collect(),
                    None if materials.classify(state.id.0).is_solid() => {
                        Box::new([mc_physics::BlockCollisionBox::FULL_BLOCK])
                    }
                    None => Box::default(),
                };
            for collision_box in &boxes {
                let height = (i32::from(collision_box.coordinates()[4]) + 255) / 256;
                max_collision_box_y =
                    max_collision_box_y.max(u8::try_from(height).map_err(|_| {
                        CoreError::Registry("collision box exceeds scan height".into())
                    })?);
            }
            max_collision_boxes_per_cell = max_collision_boxes_per_cell.max(boxes.len());
            collision_boxes[state.id.0 as usize] = boxes;
        }
        let mut blueprints = vanilla_blueprints(&entity_types, &items, &entity_loot);
        crate::species::apply_food_tags(&tags, &mut blueprints);
        let name = |id: &str| Identifier::parse(id).expect("vanilla entity identifier parses");
        let item_type_id = entity_types
            .id_of(&name("minecraft:item"))
            .ok_or_else(|| CoreError::Registry("missing minecraft:item".to_owned()))?;
        let cow_type_id = entity_types
            .id_of(&name("minecraft:cow"))
            .ok_or_else(|| CoreError::Registry("missing minecraft:cow".to_owned()))?;
        let zombie_type_id = entity_types
            .id_of(&name("minecraft:zombie"))
            .ok_or_else(|| CoreError::Registry("missing minecraft:zombie".to_owned()))?;
        let shears_item =
            items.id_of(&Identifier::parse("minecraft:shears").expect("item id parses"));
        let enchantments = crate::inventory::enchanting::EnchantmentCatalog::load(root)
            .map_err(CoreError::Registry)?;
        let advancements = Arc::new(match vanilla_data_dir {
            Some(dir) => mc_data::advancements::AdvancementTree::load(dir)
                .map_err(|error| CoreError::Registry(error.to_string()))?,
            None => mc_data::advancements::AdvancementTree::default(),
        });
        let advancement_reward_loot = match vanilla_data_dir {
            Some(dir) => crate::advancements::load_reward_loot(dir, &advancements, &items)?,
            None => Default::default(),
        };
        let advancement_blocks = (0..blocks.len())
            .map(|_| std::sync::OnceLock::new())
            .collect();
        Ok(Self {
            blocks,
            shears_item,
            items: std::sync::Arc::new(items),
            loot,
            entity_loot,
            entity_loot_lookups,
            chest_loot,
            fishing_loot,
            trial_loot,
            mining,
            explosion,
            light,
            entity_types,
            materials,
            collision_boxes,
            outlines,
            max_collision_box_y,
            max_collision_boxes_per_cell,
            states,
            blueprint_index: index_blueprints(&blueprints),
            blueprints,
            biome_spawns,
            item_type_id,
            cow_type_id,
            zombie_type_id,
            item_facts,
            tags,
            worldgen_tags,
            recipes,
            advancements,
            advancement_blocks,
            advancement_reward_loot,
            enchantments,
            farming,
            hazard_flags,
            wooden_doors,
            fire_states,
            plugin_rules: None,
            plugin_ore_profile: None,
            plugin_client_bundles: Vec::new(),
            plugin_item_ids: Vec::new(),
            plugin_work_recipe_ids: Vec::new(),
            plugin_crafting_recipes: Vec::new(),
            block_tools: crate::block_tools::BlockToolTable::default(),
        })
    }

    /// Full vanilla data from the content cache. Missing data is a startup
    /// error; there is no fallback (a partial table would silently change
    /// drops/crafting).
    fn load_full_data(
        dir: &std::path::Path,
        items: &mc_data::items::ItemRegistry,
        blocks: &Arc<BlockRegistry>,
    ) -> Result<LoadedVanillaData, CoreError> {
        let fail = |what: &str| {
            CoreError::Registry(format!("vanilla {what} missing at {}", dir.display()))
        };
        let item_facts = mc_data::item_components::load_item_facts(
            dir.join("reports")
                .join("minecraft")
                .join("components")
                .join("item"),
        )
        .map_err(|err| CoreError::Registry(err.to_string()))?;
        if item_facts.is_empty() {
            return Err(fail("item facts"));
        }
        let data =
            Arc::new(mc_data::load(dir).map_err(|err| CoreError::Registry(err.to_string()))?);
        // Resolved wire tags alone do not contain the furnace burn-time table.
        let tags = mc_data::tags::load(dir, &data)
            .map_err(|err| CoreError::Registry(err.to_string()))?
            .with_vanilla_fuel_values(items);
        if tags.total_tags() == 0 {
            return Err(fail("tags"));
        }
        let tags = Arc::new(tags);
        let worldgen_tags = Arc::new(mc_worldgen::vanilla_features::CacheTags::new(
            Arc::clone(blocks),
            data,
            Arc::clone(&tags),
        ));
        let mut loot = mc_data::loot::load_vanilla_subset(
            dir.join("data").join("minecraft").join("loot_table"),
        )
        .map_err(|err| CoreError::Registry(err.to_string()))?;
        if loot.total_drops() == 0 {
            return Err(fail("loot tables"));
        }
        let builtin_loot = mc_data::loot::load_builtin(dir)
            .map_err(|error| CoreError::Registry(error.to_string()))?;
        loot.fill_missing_from(&builtin_loot);
        let mut recipes =
            mc_data::recipes::load_recipes(dir.join("data").join("minecraft").join("recipe"))
                .map_err(|err| CoreError::Registry(err.to_string()))?;
        if recipes.is_empty() {
            return Err(fail("recipes"));
        }
        // Embedded ids stay available when the cache lacks them (v1 shape).
        let embedded = mc_data::recipes::solaris_required_recipes();
        let mut by_id: std::collections::BTreeMap<_, _> = recipes
            .into_iter()
            .map(|recipe| (recipe.id.clone(), recipe))
            .collect();
        recipes = Vec::with_capacity(embedded.len() + by_id.len());
        for fallback in embedded {
            recipes.push(by_id.remove(&fallback.id).unwrap_or(fallback));
        }
        recipes.extend(by_id.into_values());
        let mining = mc_data::block_mining::load(dir.join("reports").join("block_mining.json"))
            .map_err(|err| CoreError::Registry(err.to_string()))?;
        Ok((item_facts, tags, recipes, loot, mining, worldgen_tags))
    }

    /// Blueprint for a blueprint key, if the core simulates it. O(1): the
    /// tick reads capabilities through this on every mob.
    #[must_use]
    pub fn blueprint(&self, type_id: u32) -> Option<&MobBlueprint> {
        let at = *self.blueprint_index.get(index_slot(type_id))?;
        self.blueprints
            .get(at as usize)
            .filter(|(id, _)| *id == type_id)
            .map(|(_, blueprint)| blueprint)
    }

    /// Admit one data-defined blueprint (ADR 0016) and return its key. Any
    /// bound violation, unknown reference or duplicate id is a load error.
    pub fn register_blueprint(
        &mut self,
        definition: crate::blueprint::MobBlueprintDef,
    ) -> Result<u32, CoreError> {
        if self
            .blueprints
            .iter()
            .any(|(_, blueprint)| blueprint.name == definition.id.as_str())
        {
            return Err(CoreError::Registry(format!(
                "mob blueprint {} is already registered",
                definition.id
            )));
        }
        let ordinal = self
            .blueprints
            .iter()
            .filter(|(id, _)| id & crate::blueprint::CUSTOM_TYPE_FLAG != 0)
            .count();
        let blueprint = definition
            .admit(ordinal, &self.entity_types, &self.entity_loot)
            .map_err(CoreError::Registry)?;
        let key = blueprint.type_id;
        self.blueprints.push((key, blueprint));
        self.blueprint_index = index_blueprints(&self.blueprints);
        Ok(key)
    }

    /// Drop blueprints (fixtures that test a missing type).
    #[cfg(test)]
    pub(crate) fn retain_blueprints(&mut self, keep: impl FnMut(&(u32, MobBlueprint)) -> bool) {
        self.blueprints.retain(keep);
        self.blueprint_index = index_blueprints(&self.blueprints);
    }
}

fn index_slot(type_id: u32) -> usize {
    if type_id & crate::blueprint::CUSTOM_TYPE_FLAG == 0 {
        type_id as usize
    } else {
        mc_data::entity_types::ENTITY_TYPE_COUNT + ((type_id >> 16) & 0x7FFF) as usize
    }
}

/// First registration of a key wins, like the former linear lookup.
fn index_blueprints(blueprints: &[(u32, MobBlueprint)]) -> Vec<u32> {
    let mut index = vec![u32::MAX; mc_data::entity_types::ENTITY_TYPE_COUNT];
    for (at, (type_id, _)) in blueprints.iter().enumerate() {
        let slot = index_slot(*type_id);
        if slot >= index.len() {
            index.resize(slot + 1, u32::MAX);
        }
        if index[slot] == u32::MAX {
            index[slot] = at as u32;
        }
    }
    index
}

impl ResolvedStates {
    fn resolve(blocks: &BlockRegistry) -> Result<Self, CoreError> {
        let default_of = |name: &str| {
            let id = Identifier::parse(name).expect("vanilla block identifier parses");
            blocks
                .block(&id)
                .map(|block| block.default.0)
                .ok_or_else(|| CoreError::Registry(format!("missing {name}")))
        };
        let levels = |name: &str| -> Result<[u32; 16], CoreError> {
            let id = Identifier::parse(name).expect("vanilla block identifier parses");
            let mut out = [0; 16];
            for (level, slot) in out.iter_mut().enumerate() {
                *slot = blocks
                    .by_name_and_props(&id, &[("level".to_owned(), level.to_string())])
                    .map(|state| state.0)
                    .ok_or_else(|| CoreError::Registry(format!("missing {name} level {level}")))?;
            }
            Ok(out)
        };
        Ok(Self {
            air: default_of("minecraft:air")?,
            stone: default_of("minecraft:stone")?,
            dirt: default_of("minecraft:dirt")?,
            sand: default_of("minecraft:sand")?,
            gravel: default_of("minecraft:gravel")?,
            water: default_of("minecraft:water")?,
            torch: default_of("minecraft:torch")?,
            grass_block: default_of("minecraft:grass_block")?,
            obsidian: default_of("minecraft:obsidian")?,
            cobblestone: default_of("minecraft:cobblestone")?,
            water_levels: levels("minecraft:water")?,
            lava_levels: levels("minecraft:lava")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CoreRegistry;
    use std::sync::Arc;

    #[test]
    fn embedded_template_shares_immutable_blocks_but_isolates_fixture_edits() {
        let mut first = CoreRegistry::load(None).expect("embedded registry");
        let second = CoreRegistry::load(None).expect("embedded registry");
        assert!(Arc::ptr_eq(&first.blocks, &second.blocks));
        assert!(Arc::ptr_eq(&first.items, &second.items));
        assert!(Arc::ptr_eq(&first.light, &second.light));
        assert!(Arc::ptr_eq(&first.entity_loot, &second.entity_loot));
        assert!(Arc::ptr_eq(&first.chest_loot, &second.chest_loot));
        assert!(Arc::ptr_eq(&first.fishing_loot, &second.fishing_loot));
        assert!(Arc::ptr_eq(&first.trial_loot, &second.trial_loot));
        assert!(!first.recipes.is_empty());
        first.recipes.clear();
        Arc::make_mut(&mut first.tags).registries.clear();
        let third = CoreRegistry::load(None).expect("embedded registry");
        assert_eq!(third.recipes.len(), second.recipes.len());
        assert_eq!(third.tags.registries, second.tags.registries);
    }

    #[test]
    fn wooden_doors_table_matches_exact_families() {
        let registry = CoreRegistry::load(None).expect("embedded registry");
        let mut wooden = 0usize;
        for state in registry.blocks.states() {
            let expected = matches!(
                state.block.id.path(),
                "oak_door"
                    | "spruce_door"
                    | "birch_door"
                    | "jungle_door"
                    | "acacia_door"
                    | "dark_oak_door"
                    | "pale_oak_door"
                    | "crimson_door"
                    | "warped_door"
                    | "mangrove_door"
                    | "bamboo_door"
                    | "cherry_door"
            );
            assert_eq!(
                registry.wooden_doors.get(state.id.0 as usize).copied(),
                Some(expected),
                "wooden_doors mismatch for {}",
                state.block.id
            );
            wooden += usize::from(expected);
        }
        assert!(wooden > 0, "expected wooden door states in the registry");
        // Iron/copper doors (every oxidation/wax state) stay excluded, open or
        // closed, lower or upper.
        for name in ["minecraft:iron_door", "minecraft:copper_door"] {
            let id = mc_data::Identifier::parse(name).expect("door id parses");
            let block = registry.blocks.block(&id).expect("door block present");
            assert!(!block.states.is_empty(), "{name} has states");
            for state_id in &block.states {
                assert_eq!(
                    registry.wooden_doors.get(state_id.0 as usize).copied(),
                    Some(false),
                    "{name} state {} must stay excluded",
                    state_id.0
                );
            }
        }
        assert_eq!(registry.wooden_doors.get(usize::MAX).copied(), None);
    }
    #[test]
    fn selected_root_must_supply_catalogs_even_after_a_fixture_loaded() {
        let _fixture = CoreRegistry::load(None).expect("local vanilla fixture");
        let empty = tempfile::tempdir().unwrap();
        let error = CoreRegistry::load(Some(empty.path()))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("data/minecraft/loot_table/entities/allay.json"),
            "{error}"
        );
        assert!(
            error.contains(&empty.path().display().to_string()),
            "{error}"
        );
    }
}
