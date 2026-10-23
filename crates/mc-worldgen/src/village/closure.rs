//! The village closure: everything the village structure set reaches, loaded by
//! reference and closed against the two halves of the split.
//!
//! The JSON registries come from `mc-data::village_data` (structure set,
//! structures, template pools, processor lists, per id), the piece NBT comes
//! from [`crate::structures::StructureTemplate`] (this crate, because jigsaw
//! metadata is engine-side), and the walk between them is here: a pool names
//! pieces, a piece's jigsaw blocks name the pools they connect to, a pool names
//! its fallback, and every pool element names its processor list. Nothing
//! outside that closure is read. List elements retain their child ordering:
//! the first child provides connectors and all children are placed together.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;

use mc_data::village_data::{
    PlacementSpec, PoolElementSpec, RandomSpreadType, StructureProcessorSpec, TemplatePoolSpec,
    VillageDataError, VillageDataLoader,
};
use mc_data::{Identifier, ResourcePath};
use mc_world::BlockRegistry;
use thiserror::Error;

use crate::structures::StructureTemplate;

/// A structure of the set, with its weighting and its resolved settings.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosureStructure {
    pub id: Identifier,
    pub weight: u32,
    pub spec: mc_data::village_data::StructureSpec,
}

/// One pool element the engine can place.
#[derive(Debug, Clone, PartialEq)]
pub enum ClosureElement {
    Empty,
    Single {
        piece: Identifier,
        projection: mc_data::village_data::Projection,
        processors: mc_data::village_data::ProcessorRef,
        /// `legacy_single_pool_element` rather than `single_pool_element`:
        /// vanilla's two place settings differ (legacy also installs
        /// `BlockIgnoreProcessor.STRUCTURE_AND_AIR` last), so the kind is
        /// carried, never assumed.
        legacy: bool,
    },
    List {
        elements: Vec<ClosureElement>,
        projection: mc_data::village_data::Projection,
    },
    Feature {
        placed_feature: Identifier,
        projection: mc_data::village_data::Projection,
    },
}

/// A pool reduced to the element types the engine places.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosurePool {
    pub id: Identifier,
    pub fallback: Identifier,
    pub elements: Vec<(u32, ClosureElement)>,
    /// `StructureTemplatePool.getMaxSize`: the tallest bounding box any
    /// non-`empty_pool_element` element of the pool produces at
    /// `Rotation.NONE`, or 0 for a pool of empty elements only.
    ///
    /// `use_expansion_hack` reads it for every jigsaw of a candidate whose
    /// front position lands inside the candidate's own box, so a village pool
    /// carrying only `feature_pool_element` entries reports 1 (a feature
    /// element is a degenerate one-block box) rather than 0.
    pub max_size: i32,
}

/// Everything the village closure reached.
#[derive(Debug, Clone)]
pub struct VillageClosure {
    pub structure_set: Identifier,
    /// Dimension minimum Y and exclusive maximum Y for height anchors/padding.
    pub dimension_bounds: (i32, i32),
    pub placement: crate::village::placement::RandomSpreadPlacement,
    pub structures: Vec<ClosureStructure>,
    pub pools: BTreeMap<Identifier, ClosurePool>,
    pub processor_lists: BTreeMap<Identifier, Vec<StructureProcessorSpec>>,
    pub pieces: BTreeMap<Identifier, StructureTemplate>,
    /// The distinct placed features the closure's `feature_pool_element` entries
    /// name — the A1/A2 executor's input.
    pub placed_features: Vec<Identifier>,
}

impl VillageClosure {
    /// Whether the closure reaches `pool`.
    #[must_use]
    pub fn pool(&self, pool: &Identifier) -> Option<&ClosurePool> {
        self.pools.get(pool)
    }

    /// Whether the closure reaches `piece`.
    #[must_use]
    pub fn piece(&self, piece: &Identifier) -> Option<&StructureTemplate> {
        self.pieces.get(piece)
    }

    /// The entity types the closure's pieces place that the village lane does
    /// not spawn, in ascending id order.
    ///
    /// The lane spawns a piece's `minecraft:villager` as a chunk inhabitant
    /// marker ([`SPAWNED_PIECE_MOB`]). Every other entity a village authors — the
    /// zombie villager of a zombie village's houses, the animal pens' livestock
    /// and their cats, the meeting point's iron golem, a desert camel, an armour
    /// stand — would need its own entity state and spawn path, so the lane leaves
    /// them unplaced and startup reports them by name.
    #[must_use]
    pub fn unspawned_piece_mobs(&self) -> Vec<&str> {
        let mut mobs = self
            .pieces
            .values()
            .flat_map(StructureTemplate::entities)
            .map(|entity| entity.entity_type.as_str())
            .filter(|entity_type| *entity_type != SPAWNED_PIECE_MOB)
            .collect::<Vec<_>>();
        mobs.sort_unstable();
        mobs.dedup();
        mobs
    }
}

/// The one entity type the village lane spawns: a piece template's villager
/// becomes a chunk inhabitant marker, which the runtime turns into a villager.
pub const SPAWNED_PIECE_MOB: &str = "minecraft:villager";

#[derive(Debug, Error)]
pub enum ClosureError {
    #[error(transparent)]
    Data(#[from] VillageDataError),
    #[error(transparent)]
    Reader(#[from] std::io::Error),
    #[error(transparent)]
    Template(#[from] crate::structures::StructureError),
    #[error(transparent)]
    ResourcePath(#[from] mc_data::ResourcePathError),
}

/// Load and walk the closure from a vanilla content cache root (the directory
/// holding `data/minecraft/**`).
pub fn load_village_closure(
    cache_root: impl AsRef<Path>,
    structure_set: &Identifier,
    registry: &BlockRegistry,
) -> Result<VillageClosure, ClosureError> {
    let cache_root = cache_root.as_ref();
    let loader = VillageDataLoader::new(cache_root.join("data").join("minecraft").join("worldgen"));
    let set = loader.load_structure_set(structure_set)?;
    load_jigsaw_closure(cache_root, structure_set, set, registry)
}

pub fn load_jigsaw_closure(
    cache_root: &Path,
    structure_set: &Identifier,
    set: mc_data::village_data::StructureSetSpec,
    registry: &BlockRegistry,
) -> Result<VillageClosure, ClosureError> {
    let loader = VillageDataLoader::new(cache_root.join("data").join("minecraft").join("worldgen"));
    let mut structures = Vec::with_capacity(set.structures.len());
    for entry in &set.structures {
        let spec = loader.load_structure(&entry.structure)?;
        structures.push(ClosureStructure {
            id: entry.structure.clone(),
            weight: u32::try_from(entry.weight).unwrap_or(1),
            spec,
        });
    }

    let mut pools: BTreeMap<Identifier, ClosurePool> = BTreeMap::new();
    let mut processor_lists: BTreeMap<Identifier, Vec<StructureProcessorSpec>> = BTreeMap::new();
    let mut pieces: BTreeMap<Identifier, StructureTemplate> = BTreeMap::new();
    let mut placed_features: Vec<Identifier> = Vec::new();
    let mut queue: VecDeque<(Identifier, Identifier)> = structures
        .iter()
        .map(|structure| (structure.spec.start_pool.clone(), structure.id.clone()))
        .collect();
    for structure in &structures {
        queue_alias_targets(&structure.spec.pool_aliases, &structure.id, &mut queue);
    }

    while let Some((pool_id, _referrer)) = queue.pop_front() {
        if structures
            .iter()
            .any(|structure| alias_defined(&structure.spec.pool_aliases, &pool_id))
        {
            continue;
        }
        if pools.contains_key(&pool_id) {
            continue;
        }
        let spec: TemplatePoolSpec = loader.load_template_pool(&pool_id)?;
        let mut elements = Vec::with_capacity(spec.elements.len());
        for entry in &spec.elements {
            let element = load_element(
                &entry.element,
                &pool_id,
                cache_root,
                registry,
                &loader,
                &mut processor_lists,
                &mut pieces,
                &mut placed_features,
                &mut queue,
            )?;
            elements.push((u32::try_from(entry.weight).unwrap_or(1), element));
        }
        if !pools.contains_key(&spec.fallback) {
            queue.push_back((spec.fallback.clone(), pool_id.clone()));
        }
        let max_size = max_size_of(&elements, &pieces);
        pools.insert(
            pool_id.clone(),
            ClosurePool {
                id: pool_id,
                fallback: spec.fallback,
                elements,
                max_size,
            },
        );
    }

    let mc_data::village_data::RandomSpreadPlacement {
        spacing,
        separation,
        salt,
        spread_type,
        ..
    } = match &set.placement {
        PlacementSpec::RandomSpread(placement) => placement.clone(),
    };
    placed_features.sort();
    Ok(VillageClosure {
        structure_set: structure_set.clone(),
        dimension_bounds: (-64, 320),
        placement: crate::village::placement::RandomSpreadPlacement {
            spacing,
            separation,
            salt: i64::from(salt),
            triangular: spread_type == RandomSpreadType::Triangular,
        },
        structures,
        pools,
        processor_lists,
        pieces,
        placed_features,
    })
}

/// `StructureTemplatePool.getMaxSize`, over the elements the closure kept.
///
/// The pool's templates at `Rotation.NONE` produce the template's own height for
/// a piece element and a one-block box for a feature element
/// (`FeaturePoolElement.getBoundingBox` is `position..position`); empty elements
/// are filtered out before the maximum, and a pool with nothing else reports 0.
fn max_size_of(
    elements: &[(u32, ClosureElement)],
    pieces: &BTreeMap<Identifier, StructureTemplate>,
) -> i32 {
    elements
        .iter()
        .map(|(_, element)| element_height(element, pieces))
        .max()
        .unwrap_or(0)
}

fn element_height(
    element: &ClosureElement,
    pieces: &BTreeMap<Identifier, StructureTemplate>,
) -> i32 {
    match element {
        ClosureElement::Empty => 0,
        ClosureElement::Feature { .. } => 1,
        ClosureElement::Single { piece, .. } => {
            pieces.get(piece).map_or(1, |template| template.size()[1])
        }
        ClosureElement::List { elements, .. } => elements
            .iter()
            .map(|element| element_height(element, pieces))
            .max()
            .unwrap_or(0),
    }
}

fn alias_defined(aliases: &[mc_data::village_data::PoolAliasSpec], id: &Identifier) -> bool {
    use mc_data::village_data::PoolAliasSpec;
    aliases.iter().any(|binding| match binding {
        PoolAliasSpec::Direct { alias, .. } | PoolAliasSpec::Random { alias, .. } => alias == id,
        PoolAliasSpec::RandomGroup { groups } => {
            groups.iter().any(|(_, group)| alias_defined(group, id))
        }
    })
}

fn queue_alias_targets(
    aliases: &[mc_data::village_data::PoolAliasSpec],
    owner: &Identifier,
    queue: &mut VecDeque<(Identifier, Identifier)>,
) {
    use mc_data::village_data::PoolAliasSpec;
    for alias in aliases {
        match alias {
            PoolAliasSpec::Direct { target, .. } => {
                queue.push_back((target.clone(), owner.clone()))
            }
            PoolAliasSpec::Random { targets, .. } => queue.extend(
                targets
                    .iter()
                    .map(|(_, target)| (target.clone(), owner.clone())),
            ),
            PoolAliasSpec::RandomGroup { groups } => {
                for (_, aliases) in groups {
                    queue_alias_targets(aliases, owner, queue);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn load_element(
    element: &PoolElementSpec,
    owner: &Identifier,
    cache_root: &Path,
    registry: &BlockRegistry,
    loader: &VillageDataLoader,
    processor_lists: &mut BTreeMap<Identifier, Vec<StructureProcessorSpec>>,
    pieces: &mut BTreeMap<Identifier, StructureTemplate>,
    placed_features: &mut Vec<Identifier>,
    queue: &mut VecDeque<(Identifier, Identifier)>,
) -> Result<ClosureElement, ClosureError> {
    Ok(match element {
        PoolElementSpec::Empty => ClosureElement::Empty,
        PoolElementSpec::Single(single) | PoolElementSpec::LegacySingle(single) => {
            load_processors(loader, &single.processors, processor_lists)?;
            if !pieces.contains_key(&single.location) {
                let template = load_piece(cache_root, &single.location, owner, registry)?;
                queue.extend(
                    template
                        .jigsaws()
                        .iter()
                        .map(|jigsaw| (jigsaw.pool.clone(), single.location.clone())),
                );
                pieces.insert(single.location.clone(), template);
            }
            ClosureElement::Single {
                piece: single.location.clone(),
                projection: single.projection,
                processors: single.processors.clone(),
                legacy: matches!(element, PoolElementSpec::LegacySingle(_)),
            }
        }
        PoolElementSpec::Feature {
            feature,
            projection,
        } => {
            if !placed_features.contains(feature) {
                placed_features.push(feature.clone());
            }
            ClosureElement::Feature {
                placed_feature: feature.clone(),
                projection: *projection,
            }
        }
        PoolElementSpec::List {
            elements,
            projection,
        } => ClosureElement::List {
            elements: elements
                .iter()
                .map(|child| {
                    let mut child = load_element(
                        child,
                        owner,
                        cache_root,
                        registry,
                        loader,
                        processor_lists,
                        pieces,
                        placed_features,
                        queue,
                    )?;
                    child.set_projection(*projection);
                    Ok(child)
                })
                .collect::<Result<_, ClosureError>>()?,
            projection: *projection,
        },
    })
}

impl ClosureElement {
    fn set_projection(&mut self, value: mc_data::village_data::Projection) {
        match self {
            Self::Single { projection, .. } | Self::Feature { projection, .. } => {
                *projection = value
            }
            Self::List {
                elements,
                projection,
            } => {
                *projection = value;
                for child in elements {
                    child.set_projection(value);
                }
            }
            Self::Empty => {}
        }
    }

    /// Ordered terminal children used for validation and template placement.
    pub fn leaves<'a>(&'a self, out: &mut Vec<&'a Self>) {
        if let Self::List { elements, .. } = self {
            for child in elements {
                child.leaves(out);
            }
        } else {
            out.push(self);
        }
    }
}

fn load_processors(
    loader: &VillageDataLoader,
    processors: &mc_data::village_data::ProcessorRef,
    out: &mut BTreeMap<Identifier, Vec<StructureProcessorSpec>>,
) -> Result<(), ClosureError> {
    match processors {
        mc_data::village_data::ProcessorRef::List(list) => {
            if !out.contains_key(list) {
                let spec = loader.load_processor_list(list)?;
                out.insert(list.clone(), spec.processors);
            }
        }
        // Inline lists are per element and already carried on the element, so
        // only named lists enter the closure's list map.
        mc_data::village_data::ProcessorRef::Inline(_) => {}
    }
    Ok(())
}

fn load_piece(
    cache_root: &Path,
    piece: &Identifier,
    referrer: &Identifier,
    registry: &BlockRegistry,
) -> Result<StructureTemplate, ClosureError> {
    let root = cache_root.join("data").join("minecraft").join("structure");
    let mut resource = ResourcePath::from_identifier_path(piece)?;
    resource.set_extension("nbt")?;
    let path = resource.lexical_under(&root);
    match StructureTemplate::from_nbt_file(&path, registry) {
        Ok(template) => Ok(template),
        Err(crate::structures::StructureError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            // Vanilla getOrCreate uses a zero-sized template for absent
            // resources. The closure caches it, so each id is reported once.
            tracing::warn!(template = %piece, referrer = %referrer,
                "missing jigsaw template; using vanilla empty-template semantics");
            Ok(StructureTemplate::new([0; 3], Vec::new()))
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
#[path = "closure_tests.rs"]
mod tests;
