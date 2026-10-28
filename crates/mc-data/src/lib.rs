//! # mc-data
//!
//! Read-only indexes into the vanilla data sidecar described in
//! [`docs/decisions/0001-vanilla-data-as-runtime-input.md`]. Registry
//! entries are discovered from `<data_dir>/data/<namespace>/<registry>/**/*.json`;
//! selected reports, tags, recipes, item facts, block facts, and worldgen
//! facts are parsed by focused loaders.
//!
//! Concrete file layout (per registry):
//!
//! ```text
//! <data_dir>/data/<namespace>/<registry-path>/<sub>/.../<name>.json
//!                       ^ namespace, e.g. "minecraft"
//!                                   ^ registry, e.g. "dimension_type" or
//!                                                    "worldgen/biome"
//! ```
//!
//! The registry's *path* may be nested (e.g. `worldgen/biome`); the
//! entries inside may also be nested (e.g. recipes use subfolders).
//! Both are flattened: an entry's `Identifier` path is the part *after*
//! the registry root, with `/` separators and no `.json` suffix —
//! exactly what vanilla uses on the wire.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use thiserror::Error;
use tracing::{debug, trace};

const REQUIRED_REGISTRY_INDEX: &str = include_str!("../data/required_registry_index.json");
const MAX_JSON_WALK_DEPTH: usize = 32;
const MAX_JSON_WALK_ENTRIES: usize = 131_072;
const MAX_JSON_FILES: usize = 65_536;
const MAX_JSON_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_JSON_TOTAL_BYTES: usize = 128 * 1024 * 1024;
static REQUIRED_REGISTRY_ENTRIES: OnceLock<BTreeMap<String, Vec<Identifier>>> = OnceLock::new();

pub mod advancements;
pub mod armor;
pub mod biomes;
pub mod block_entity_kinds;
pub mod block_entity_types;
pub mod block_explosion;
pub mod block_facts;
pub mod block_light;
pub mod block_mining;
pub mod block_placement_26_1_2;
pub mod block_semantics_26_1_2;
pub mod block_tools;
pub mod blocks;
pub mod collision_shapes;
pub mod conduit_nbt;
pub mod damage_types;
pub mod data_component_types_26_1_2;
pub mod enchanting_26_1_2;
mod entity_contract_26_1_2;
pub mod entity_nbt;
pub mod entity_types;
pub mod fire_flammability;
pub mod food;
pub mod fuel_values;
pub mod game_events;
pub mod identifier;
pub mod inventory_semantics_26_1_2;
pub(crate) mod item_component_values;
pub mod item_components;
pub mod item_nbt;
pub mod item_profiles;
pub mod item_semantics_26_1_2;
pub mod item_stack;
pub mod items;
pub mod legacy_26_1_2;
pub mod loot;
pub mod mob_behavior_26_1_2;
pub mod mob_effects_26_1_2;
pub mod outline_shapes;
pub mod player_nbt;
pub mod recipes;
pub mod resource_path;
mod runtime_resources;
mod sidecar;
pub mod stations;
pub mod statistics;
pub mod storage_blocks;
pub mod tags;
pub mod trial_loot;
pub mod trial_nbt;
pub mod vanilla_feature_closure;
pub mod village_data;
pub mod villager_trades_26_1_2;
pub mod worldgen_features;
pub mod worldgen_inventory;
pub mod worldgen_ores;
pub mod worldgen_structures;

pub use identifier::{Identifier, IdentifierError};
pub use item_stack::ItemStack;
pub use resource_path::{OpenedResource, ResourcePath, ResourcePathError};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// All vanilla registries Solaris needs to surface in the Configuration
/// state. Keep in sync with `tools/extract-vanilla-data.sh`.
///
/// `(registry_id_path, fs_subpath_under_data_minecraft)` — the two are
/// identical for everything except `worldgen/biome`, which protocol
/// names as `minecraft:worldgen/biome` and stores under `worldgen/biome/`.
pub const KNOWN_REGISTRIES: &[(&str, &str)] = &[
    ("banner_pattern", "banner_pattern"),
    ("cat_sound_variant", "cat_sound_variant"),
    ("cat_variant", "cat_variant"),
    ("chat_type", "chat_type"),
    ("chicken_sound_variant", "chicken_sound_variant"),
    ("chicken_variant", "chicken_variant"),
    ("cow_sound_variant", "cow_sound_variant"),
    ("cow_variant", "cow_variant"),
    ("damage_type", "damage_type"),
    ("dialog", "dialog"),
    ("dimension_type", "dimension_type"),
    ("enchantment", "enchantment"),
    ("frog_variant", "frog_variant"),
    ("instrument", "instrument"),
    ("jukebox_song", "jukebox_song"),
    ("painting_variant", "painting_variant"),
    ("pig_sound_variant", "pig_sound_variant"),
    ("pig_variant", "pig_variant"),
    ("test_environment", "test_environment"),
    ("test_instance", "test_instance"),
    ("timeline", "timeline"),
    ("trim_material", "trim_material"),
    ("trim_pattern", "trim_pattern"),
    ("wolf_sound_variant", "wolf_sound_variant"),
    ("wolf_variant", "wolf_variant"),
    ("world_clock", "world_clock"),
    ("worldgen/biome", "worldgen/biome"),
    ("zombie_nautilus_variant", "zombie_nautilus_variant"),
];

/// Exact Network-NBT payloads captured from vanilla after declining its known pack.
pub const NETWORK_REGISTRY_PAYLOAD_DIR: &str = "reports/registry_network_nbt";

pub fn network_registry_payload_path(
    root: &Path,
    registry: &Identifier,
    entry: &Identifier,
) -> Result<PathBuf, ResourcePathError> {
    Ok(network_registry_payload_resource_path(registry, entry)?.lexical_under(root))
}

fn network_registry_payload_resource_path(
    registry: &Identifier,
    entry: &Identifier,
) -> Result<ResourcePath, ResourcePathError> {
    let mut resource = ResourcePath::new();
    resource.push_static(NETWORK_REGISTRY_PAYLOAD_DIR)?;
    resource.push_namespace(registry)?;
    resource.push_identifier_path(registry)?;
    resource.push_namespace(entry)?;
    resource.push_identifier_path(entry)?;
    resource.set_extension("nbt")?;
    Ok(resource)
}

#[derive(Debug, Error)]
pub enum DataError {
    #[error(
        "vanilla data directory does not exist at {0}; \
         did you run tools/extract-vanilla-data.sh?"
    )]
    Missing(PathBuf),

    #[error("required registry {registry} is empty or missing at {path}")]
    EmptyRegistry { registry: String, path: PathBuf },

    #[error("entry id {entry:?} (from {path}) is not a valid identifier path")]
    InvalidEntry { entry: String, path: PathBuf },

    #[error(transparent)]
    ResourcePath(#[from] ResourcePathError),

    #[error("filesystem error walking {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("captured registry Network-NBT is invalid at {path}: {source}")]
    RegistryPayloadNbt {
        path: PathBuf,
        #[source]
        source: mc_nbt::NbtError,
    },

    #[error("captured registry Network-NBT has trailing bytes at {path}")]
    RegistryPayloadTrailing { path: PathBuf },

    #[error(
        "registry {registry} defines {entry} under data/minecraft but the captured \
         RegistryData payloads at {path} have no such entry"
    )]
    DefinitionWithoutPayload {
        registry: String,
        entry: String,
        path: PathBuf,
    },
}

/// One built-in registry — the registry name plus the sorted list of
/// entry identifiers found on disk.
#[derive(Debug, Clone)]
pub struct Registry {
    /// `minecraft:dimension_type`, `minecraft:worldgen/biome`, …
    pub id: Identifier,
    /// Entry identifiers in lexicographic order. `minecraft:overworld`,
    /// `minecraft:plains`, etc. Vanilla loads in registration order,
    /// which differs; per ADR 0001 we ship our own ordering and use it
    /// consistently in every packet we emit.
    pub entries: Vec<Identifier>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ItemEnchantment {
    pub id: Identifier,
    pub level: i32,
}

/// The whole vanilla-data sidecar, indexed.
#[derive(Debug, Clone)]
pub struct VanillaData {
    root: PathBuf,
    registries: BTreeMap<String, Registry>,
    registry_payloads: BTreeMap<String, BTreeMap<String, Arc<[u8]>>>,
    sidecar_root: bool,
}

impl VanillaData {
    /// Path the registries were loaded from. Empty for in-memory
    /// constructions (e.g. test stubs).
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Filesystem sidecar root when this index came from [`load`]. Embedded or
    /// in-memory test registries are not sidecar data even if their diagnostic
    /// root string happens to name an existing path.
    #[must_use]
    pub fn sidecar_root(&self) -> Option<&Path> {
        self.sidecar_root.then_some(self.root.as_path())
    }

    /// Iterate every loaded registry in registry-id order.
    pub fn registries(&self) -> impl Iterator<Item = &Registry> {
        self.registries.values()
    }

    /// Look up a registry by its id (`"minecraft:dimension_type"`,
    /// without the namespace prefix, or with — we accept both).
    #[must_use]
    pub fn registry(&self, name: &str) -> Option<&Registry> {
        let key = name.strip_prefix("minecraft:").unwrap_or(name);
        self.registries.get(key)
    }

    /// Number of indexed registries.
    #[must_use]
    pub fn registry_count(&self) -> usize {
        self.registries.len()
    }

    /// Total number of entries across all registries.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.registries.values().map(|r| r.entries.len()).sum()
    }

    /// Pre-encoded Network-NBT for one sidecar registry entry. In-memory and
    /// embedded indexes intentionally return `None`: they carry identifiers,
    /// not invented data-pack definitions.
    #[must_use]
    pub fn registry_entry_payload(
        &self,
        registry: &Identifier,
        entry: &Identifier,
    ) -> Option<Arc<[u8]>> {
        self.registry_payloads
            .get(registry.as_str())?
            .get(entry.as_str())
            .map(Arc::clone)
    }

    /// Whether every indexed registry entry has a sidecar-backed payload.
    #[must_use]
    pub fn has_full_registry_payloads(&self) -> bool {
        self.entry_count() > 0
            && self.registries.values().all(|registry| {
                self.registry_payloads
                    .get(registry.id.as_str())
                    .is_some_and(|payloads| {
                        payloads.len() == registry.entries.len()
                            && registry
                                .entries
                                .iter()
                                .all(|entry| payloads.contains_key(entry.as_str()))
                    })
            })
    }

    /// Build a `VanillaData` from in-memory registries. Used by tests
    /// that don't want to stage a filesystem layout and by future code
    /// that loads from somewhere other than disk. `root` is recorded
    /// only for diagnostic purposes.
    #[must_use]
    pub fn from_registries(root: impl Into<PathBuf>, registries: Vec<Registry>) -> Self {
        let map = registries
            .into_iter()
            .map(|r| (r.id.path().to_string(), r))
            .collect();
        Self {
            root: root.into(),
            registries: map,
            registry_payloads: BTreeMap::new(),
            sidecar_root: false,
        }
    }
}

/// Helpers used only from tests. Public so integration tests in other
/// crates can build a `VanillaData` without staging a filesystem layout.
#[doc(hidden)]
pub mod testing {
    use super::{Identifier, KNOWN_REGISTRIES, Registry, VanillaData};

    /// A minimal `VanillaData` with every known registry populated with
    /// two placeholder entries. Useful as a stub for tests that exercise
    /// the network handler without caring about specific entry contents.
    #[must_use]
    pub fn stub() -> VanillaData {
        let registries = KNOWN_REGISTRIES
            .iter()
            .map(|(path, _)| {
                let id = Identifier::parse(format!("minecraft:{path}"))
                    .expect("KNOWN_REGISTRIES paths are valid identifiers");
                let entries = vec![
                    Identifier::parse("minecraft:alpha").unwrap(),
                    Identifier::parse("minecraft:beta").unwrap(),
                ];
                Registry { id, entries }
            })
            .collect();
        VanillaData::from_registries("", registries)
    }
}

/// Repo-owned registry index used when the local vanilla sidecar is absent.
/// Entries are the minimal identifiers Solaris needs to complete the
/// Configuration state and keep worldgen/survival baselines running.
#[must_use]
pub fn solaris_required_data() -> VanillaData {
    let index = required_registry_entries();

    let registries = KNOWN_REGISTRIES
        .iter()
        .map(|(path, _)| {
            let entries = index
                .get(*path)
                .unwrap_or_else(|| panic!("embedded required registry index missing {path}"))
                .to_vec();
            Registry {
                id: Identifier::parse(format!("minecraft:{path}"))
                    .expect("KNOWN_REGISTRIES paths are valid identifiers"),
                entries,
            }
        })
        .collect();
    VanillaData::from_registries("<solaris-built-in>", registries)
}

#[must_use]
pub fn required_registry_entry_id(registry: &str, entry: &Identifier) -> Option<u32> {
    let registry = registry.strip_prefix("minecraft:").unwrap_or(registry);
    required_registry_entries()
        .get(registry)?
        .binary_search(entry)
        .ok()
        .and_then(|id| u32::try_from(id).ok())
}

#[must_use]
pub fn required_registry_entry(registry: &str, protocol_id: u32) -> Option<Identifier> {
    let registry = registry.strip_prefix("minecraft:").unwrap_or(registry);
    required_registry_entries()
        .get(registry)?
        .get(protocol_id as usize)
        .cloned()
}

fn required_registry_entries() -> &'static BTreeMap<String, Vec<Identifier>> {
    REQUIRED_REGISTRY_ENTRIES.get_or_init(|| {
        let raw: BTreeMap<String, Vec<String>> = serde_json::from_str(REQUIRED_REGISTRY_INDEX)
            .expect("embedded required registry index JSON is valid");
        raw.into_iter()
            .map(|(registry, entries)| {
                let entries = entries
                    .into_iter()
                    .map(|entry| {
                        Identifier::parse(entry)
                            .expect("embedded required registry entry id is valid")
                    })
                    .collect();
                (registry, entries)
            })
            .collect()
    })
}

fn load_registry_payload(opened: OpenedResource) -> Result<Vec<u8>, DataError> {
    let (path, file) = opened.into_parts();
    let bytes = sidecar::read_opened_file(&path, file).map_err(|source| DataError::Io {
        path: path.clone(),
        source,
    })?;
    let mut remaining = bytes.as_slice();
    let _ =
        mc_nbt::read_network(&mut remaining).map_err(|source| DataError::RegistryPayloadNbt {
            path: path.clone(),
            source,
        })?;
    if !remaining.is_empty() {
        return Err(DataError::RegistryPayloadTrailing { path });
    }
    Ok(bytes)
}

/// Load the vanilla data sidecar rooted at `path` (typically
/// `data/vanilla/`).
///
/// With captured RegistryData payloads the wire registry is exactly the
/// payload set (`reports/registry_network_nbt/minecraft/<registry>/**/*.nbt`):
/// that is what the client receives and what every numeric id indexes. The
/// gameplay definitions under `<path>/data/minecraft/<registry>/**/*.json`
/// may come from an older data pack, so they must name a subset of it.
/// Without payloads the definitions are the registry. Registries that have
/// zero entries are an error, because that means either the operator forgot
/// to run the extraction script or this Mojang version dropped a registry we
/// still expect.
pub fn load(path: impl Into<PathBuf>) -> Result<VanillaData, DataError> {
    let root = path.into();
    if !root.is_dir() {
        return Err(DataError::Missing(root));
    }
    let mc_root = root.join("data").join("minecraft");
    let has_network_payloads = root.join(NETWORK_REGISTRY_PAYLOAD_DIR).is_dir();

    let mut registries = BTreeMap::new();
    let mut registry_payloads = BTreeMap::new();
    for (registry_path, fs_subpath) in KNOWN_REGISTRIES {
        let dir = mc_root.join(fs_subpath);
        if !dir.is_dir() {
            return Err(DataError::EmptyRegistry {
                registry: (*registry_path).to_string(),
                path: dir,
            });
        }
        let mut entries = Vec::new();
        collect_entries(&root, &dir, "json", &mut entries)?;
        if has_network_payloads {
            let payload_dir = root
                .join(NETWORK_REGISTRY_PAYLOAD_DIR)
                .join("minecraft")
                .join(registry_path)
                .join("minecraft");
            let mut wire = Vec::new();
            collect_entries(&root, &payload_dir, "nbt", &mut wire)?;
            wire.sort();
            if let Some(entry) = entries
                .iter()
                .find(|entry| wire.binary_search(entry).is_err())
            {
                return Err(DataError::DefinitionWithoutPayload {
                    registry: (*registry_path).to_string(),
                    entry: format!("minecraft:{entry}"),
                    path: payload_dir,
                });
            }
            entries = wire;
        }
        if entries.is_empty() {
            return Err(DataError::EmptyRegistry {
                registry: (*registry_path).to_string(),
                path: dir,
            });
        }
        entries.sort();

        let registry_id = Identifier::parse(format!("minecraft:{registry_path}"))
            .expect("KNOWN_REGISTRIES paths are well-formed");
        let mut identifiers = Vec::with_capacity(entries.len());
        let mut payloads = BTreeMap::new();
        for stem in entries {
            let qualified = format!("minecraft:{stem}");
            let identifier =
                Identifier::parse(qualified.clone()).map_err(|_| DataError::InvalidEntry {
                    entry: qualified,
                    path: dir.clone(),
                })?;
            if has_network_payloads {
                let payload_resource =
                    network_registry_payload_resource_path(&registry_id, &identifier)?;
                let Some(payload_file) = payload_resource.open_existing_under(&root)? else {
                    let path = payload_resource.lexical_under(&root);
                    return Err(DataError::Io {
                        path: path.clone(),
                        source: std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            format!("captured registry payload missing at {}", path.display()),
                        ),
                    });
                };
                let payload = load_registry_payload(payload_file)?;
                payloads.insert(identifier.as_str().to_string(), payload.into());
            }
            identifiers.push(identifier);
        }
        debug!(
            registry = %registry_id,
            entries = identifiers.len(),
            "loaded registry"
        );
        if has_network_payloads {
            registry_payloads.insert(registry_id.as_str().to_string(), payloads);
        }
        registries.insert(
            (*registry_path).to_string(),
            Registry {
                id: registry_id,
                entries: identifiers,
            },
        );
    }

    Ok(VanillaData {
        root,
        registries,
        registry_payloads,
        sidecar_root: true,
    })
}

pub(crate) fn visit_json_files<E>(
    dir: &Path,
    visitor: &mut impl FnMut(PathBuf) -> Result<(), E>,
    io_error: &impl Fn(PathBuf, std::io::Error) -> E,
) -> Result<(), E> {
    let paths = sidecar::collect_files(dir, "json", true)
        .map_err(|error| io_error(error.path, error.source))?;
    for path in paths {
        visitor(path)?;
    }
    Ok(())
}

pub(crate) fn sorted_json_files<E>(
    dir: &Path,
    io_error: &impl Fn(PathBuf, std::io::Error) -> E,
) -> Result<Vec<PathBuf>, E> {
    sidecar::collect_files(dir, "json", false).map_err(|error| io_error(error.path, error.source))
}

pub(crate) fn read_json_file<T, E>(
    path: &Path,
    io_error: &impl Fn(PathBuf, std::io::Error) -> E,
    parse_error: &impl Fn(PathBuf, serde_json::Error) -> E,
) -> Result<T, E>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let bytes = sidecar::read_file(path).map_err(|source| io_error(path.to_path_buf(), source))?;
    serde_json::from_slice(&bytes).map_err(|source| parse_error(path.to_path_buf(), source))
}

pub(crate) enum WorldgenEntry {
    Present(OpenedResource),
    Missing(PathBuf),
}

/// Shared open step for the worldgen-by-reference loaders
/// (`vanilla_feature_closure`, `village_data`): resolve `id` to
/// `<worldgen_dir>/<dir>/<path>.json` and open it only if it stays under
/// the trusted root. Absence yields the exact lexical path the loader
/// reports in its own `MissingEntry` error.
pub(crate) fn open_worldgen_entry(
    worldgen_dir: &Path,
    dir: &str,
    id: &Identifier,
) -> Result<WorldgenEntry, ResourcePathError> {
    let root = worldgen_dir.join(dir);
    let mut resource = ResourcePath::from_identifier_path(id)?;
    resource.set_extension("json")?;
    let lexical_path = resource.lexical_under(&root);
    match resource.open_existing_under(&root)? {
        Some(opened) => Ok(WorldgenEntry::Present(opened)),
        None => Ok(WorldgenEntry::Missing(lexical_path)),
    }
}

pub(crate) fn read_json_resource<T, E>(
    opened: OpenedResource,
    io_error: &impl Fn(PathBuf, std::io::Error) -> E,
    parse_error: &impl Fn(PathBuf, serde_json::Error) -> E,
) -> Result<T, E>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let (path, file) = opened.into_parts();
    let bytes =
        sidecar::read_opened_file(&path, file).map_err(|source| io_error(path.clone(), source))?;
    serde_json::from_slice(&bytes).map_err(|source| parse_error(path, source))
}

fn collect_entries(
    trusted_root: &Path,
    root: &Path,
    extension: &str,
    out: &mut Vec<String>,
) -> Result<(), DataError> {
    let paths =
        sidecar::collect_files_under(trusted_root, root, extension, true).map_err(|error| {
            DataError::Io {
                path: error.path,
                source: error.source,
            }
        })?;
    for path in paths {
        let rel = path
            .strip_prefix(root)
            .expect("recursive walk yields paths under root");
        let stem = rel.with_extension("");
        let mut joined = String::new();
        for component in stem.components() {
            if !joined.is_empty() {
                joined.push('/');
            }
            match component.as_os_str().to_str() {
                Some(s) => joined.push_str(s),
                None => {
                    return Err(DataError::InvalidEntry {
                        entry: stem.to_string_lossy().into_owned(),
                        path: path.clone(),
                    });
                }
            }
        }
        trace!(file = %path.display(), entry = %joined, "indexed");
        out.push(joined);
    }
    Ok(())
}

#[cfg(test)]
#[path = "data_tests.rs"]
mod tests;
