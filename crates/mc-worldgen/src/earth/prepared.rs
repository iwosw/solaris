//! Offline publication: durable physical root first, committed manifest last.
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mc_world::atomic_file::{replace_file_durable, sync_parent_dir};
use mc_world::native::{NativeError, NativeReadOnlyStore, NativeStore};
use mc_world::storage::{WorldError, WorldRootLease, acquire_exclusive_world_root_lease};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::definition::{
    DefinitionInput, PlaneBounds, PlayableBounds, WorldDefinition, WorldIdentity,
    algorithm_artifact_id, hash,
};
use super::world_plan::WorldPlan;

pub const WORLD_MANIFEST: &str = "world.manifest.toml";
const STAGING: &str = ".world-prepare";
const ARTIFACTS: &str = "world-artifacts";
const DESCRIPTOR: &str = "prepared.toml";
const ROOT: &str = "root.plan";
const NATIVE: &str = "state.redb";
const BOUNDS: &str = "playable.toml";
const MAX_MANIFEST_BYTES: u64 = 16 * 1024;

#[derive(Debug, Error)]
pub enum PrepareError {
    #[error("world preparation IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("world lease: {0}")]
    Lease(#[from] WorldError),
    #[error(
        "native state: {0}; restore the committed store for this world identity, never regenerate gameplay state"
    )]
    Native(#[from] NativeError),
    #[error("invalid world definition/artifact: {0}")]
    Invalid(&'static str),
    #[error(
        "world identity mismatch; extent/resolution/seed/scale/content changes require a new world"
    )]
    IdentityMismatch,
    #[error(
        "committed world {identity}: {reason}; restore its matching root/state/operating metadata from backup"
    )]
    Committed { identity: String, reason: String },
    #[error("world manifest decode: {0}")]
    Decode(#[from] toml::de::Error),
    #[error("world manifest encode: {0}")]
    Encode(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, Copy)]
pub struct PrepareOptions {
    pub native_cache_bytes: usize,
    pub build_scratch_limit_bytes: u64,
}
impl Default for PrepareOptions {
    fn default() -> Self {
        Self {
            native_cache_bytes: 16 * 1024 * 1024,
            build_scratch_limit_bytes: 256 * 1024 * 1024,
        }
    }
}
impl PrepareOptions {
    fn validate(self, definition: &WorldDefinition) -> Result<(), PrepareError> {
        if self.native_cache_bytes == 0 {
            return Err(PrepareError::Invalid(
                "native cache budget must be positive",
            ));
        }
        if self.build_scratch_limit_bytes < definition.estimate()?.builder_peak_bytes {
            return Err(PrepareError::Invalid(
                "scratch budget below conservative builder bound; precision will not be reduced",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    #[serde(default = "legacy_native_schema")]
    native_schema: u32,
    definition: DefinitionInput,
    identity: WorldIdentity,
    algorithm_artifact: [u8; 32],
    root_hash: [u8; 32],
    root_payload_bytes: u64,
    native_store_identity: WorldIdentity,
}

fn legacy_native_schema() -> u32 {
    1
}

impl Manifest {
    fn new(definition: &WorldDefinition, root_bytes: &[u8]) -> Result<Self, PrepareError> {
        let root_hash = hash(root_bytes);
        let algorithm_artifact = algorithm_artifact_id();
        let identity = WorldIdentity::derive(definition, algorithm_artifact, root_hash)?;
        Ok(Self {
            schema: 2,
            native_schema: mc_world::native::NATIVE_PROTOTYPE_SCHEMA,
            definition: definition.input().clone(),
            identity,
            algorithm_artifact,
            root_hash,
            root_payload_bytes: root_bytes.len() as u64,
            native_store_identity: identity,
        })
    }
    fn validate(
        &self,
        expected: Option<&WorldDefinition>,
    ) -> Result<WorldDefinition, PrepareError> {
        if self.native_schema != mc_world::native::NATIVE_PROTOTYPE_SCHEMA {
            return Err(PrepareError::Native(NativeError::UnsupportedSchema(
                self.native_schema,
            )));
        }
        if self.schema != 2 || self.native_store_identity != self.identity {
            return Err(PrepareError::Invalid(
                "manifest schema/state-store identity mismatch",
            ));
        }
        let definition = self.definition.clone().validate()?;
        if self.root_payload_bytes == 0
            || self.root_payload_bytes > definition.estimate()?.root_payload_limit_bytes
        {
            return Err(PrepareError::Invalid(
                "manifest root length exceeds admitted bound",
            ));
        }
        if expected.is_some_and(|value| value != &definition)
            || self.algorithm_artifact != algorithm_artifact_id()
            || WorldIdentity::derive(&definition, self.algorithm_artifact, self.root_hash)?
                != self.identity
        {
            return Err(PrepareError::IdentityMismatch);
        }
        definition.check_preparation_support()?;
        Ok(definition)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PublicationState {
    Unprepared,
    OrphanStaging,
    OrphanPrepared,
    Committed,
}

pub fn publication_state(root: &Path) -> PublicationState {
    if root.join(WORLD_MANIFEST).exists() {
        PublicationState::Committed
    } else if root.join(ARTIFACTS).exists() {
        PublicationState::OrphanPrepared
    } else if root.join(STAGING).exists() {
        PublicationState::OrphanStaging
    } else {
        PublicationState::Unprepared
    }
}

/// A checked offline report, not a world owner. Native bytes are opened read-only.
pub struct WorldInspection {
    pub definition: WorldDefinition,
    pub identity: WorldIdentity,
    pub root_plan: WorldPlan,
    pub root_payload_bytes: u64,
    pub playable: PlayableBounds,
    pub counts: mc_world::native::NativeRecordCounts,
}

/// Read-only native composition owner; the lease is retained until it is dropped.
pub struct PreparedReadOnlyWorld {
    store: NativeReadOnlyStore,
    inspection: WorldInspection,
    _lease: Arc<WorldRootLease>,
}

impl PreparedReadOnlyWorld {
    pub fn open(
        directory: &Path,
        expected: Option<&WorldDefinition>,
        options: PrepareOptions,
    ) -> Result<Self, PrepareError> {
        let lease = acquire_exclusive_world_root_lease(directory)?;
        let manifest: Manifest = read_toml(&directory.join(WORLD_MANIFEST))?;
        let definition = manifest.validate(expected)?;
        options.validate(&definition)?;
        let artifact_dir = directory.join(ARTIFACTS);
        let checked = || -> Result<_, PrepareError> {
            let (root_plan, playable) =
                load_canonical_artifacts(&artifact_dir, &manifest, &definition)?;
            let store = NativeReadOnlyStore::open_with_cache(
                &artifact_dir.join(NATIVE),
                manifest.native_store_identity.bytes(),
                options.native_cache_bytes,
            )?;
            let counts = store.record_counts()?;
            Ok((root_plan, playable, store, counts))
        };
        let (root_plan, playable, store, counts) =
            checked().map_err(|error| PrepareError::Committed {
                identity: manifest.identity.hex(),
                reason: error.to_string(),
            })?;
        Ok(Self {
            store,
            inspection: WorldInspection {
                definition,
                identity: manifest.identity,
                root_plan,
                root_payload_bytes: manifest.root_payload_bytes,
                playable,
                counts,
            },
            _lease: lease,
        })
    }

    pub fn inspection(&self) -> &WorldInspection {
        &self.inspection
    }

    pub fn store(&self) -> &NativeReadOnlyStore {
        &self.store
    }
}

pub fn inspect_committed(
    directory: &Path,
    expected: Option<&WorldDefinition>,
    options: PrepareOptions,
) -> Result<WorldInspection, PrepareError> {
    Ok(PreparedReadOnlyWorld::open(directory, expected, options)?.inspection)
}

/// Holds the existing exclusive lease for the entire offline operation.
pub struct PreparedWorld {
    store: Arc<NativeStore>,
    definition: WorldDefinition,
    identity: WorldIdentity,
    root_plan: WorldPlan,
    root_payload_bytes: u64,
    playable: PlayableBounds,
    directory: PathBuf,
    _lease: Arc<WorldRootLease>,
}

impl PreparedWorld {
    pub fn open(
        directory: &Path,
        expected: Option<&WorldDefinition>,
        options: PrepareOptions,
    ) -> Result<Self, PrepareError> {
        let lease = acquire_exclusive_world_root_lease(directory)?;
        Self::open_leased(directory, expected, options, lease)
    }
    fn open_leased(
        directory: &Path,
        expected: Option<&WorldDefinition>,
        options: PrepareOptions,
        lease: Arc<WorldRootLease>,
    ) -> Result<Self, PrepareError> {
        let manifest: Manifest = read_toml(&directory.join(WORLD_MANIFEST))?;
        let definition = manifest.validate(expected)?;
        options.validate(&definition)?;
        let artifact_dir = directory.join(ARTIFACTS);
        let (root_plan, playable, store) =
            load_artifacts(&artifact_dir, &manifest, &definition, options).map_err(|error| {
                PrepareError::Committed {
                    identity: manifest.identity.hex(),
                    reason: error.to_string(),
                }
            })?;
        Ok(Self {
            store: Arc::new(store),
            definition,
            identity: manifest.identity,
            root_plan,
            root_payload_bytes: manifest.root_payload_bytes,
            playable,
            directory: directory.to_path_buf(),
            _lease: lease,
        })
    }
    pub fn definition(&self) -> &WorldDefinition {
        &self.definition
    }
    pub fn identity(&self) -> WorldIdentity {
        self.identity
    }
    pub fn root_plan(&self) -> &WorldPlan {
        &self.root_plan
    }
    pub fn root_payload_bytes(&self) -> u64 {
        self.root_payload_bytes
    }
    pub fn playable(&self) -> PlayableBounds {
        self.playable
    }
    pub fn store(&self) -> &NativeStore {
        &self.store
    }

    pub fn set_bounds(&mut self, bounds: PlaneBounds) -> Result<PlayableBounds, PrepareError> {
        let operating_revision = self
            .playable
            .operating_revision
            .checked_add(1)
            .ok_or(PrepareError::Invalid("operating revision exhausted"))?;
        let playable = PlayableBounds {
            bounds,
            operating_revision,
        };
        playable.validate(&self.definition)?;
        write_atomic_toml(&self.directory.join(ARTIFACTS).join(BOUNDS), &playable)?;
        self.playable = playable;
        Ok(playable)
    }
}

/// Creates a physical plane root (or explicit fixture) and empty native store, not chunks.
/// Verified orphans are resumable only for the exact same definition/bounds.
pub fn prepare(
    directory: &Path,
    definition: &WorldDefinition,
    bounds: PlaneBounds,
    options: PrepareOptions,
) -> Result<PreparedWorld, PrepareError> {
    prepare_observed(directory, definition, bounds, options, |_| Ok(()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreparePhase {
    Descriptor,
    Root,
    Store,
    Artifacts,
    Manifest,
}

fn prepare_observed(
    directory: &Path,
    definition: &WorldDefinition,
    bounds: PlaneBounds,
    options: PrepareOptions,
    mut observed: impl FnMut(PreparePhase) -> Result<(), PrepareError>,
) -> Result<PreparedWorld, PrepareError> {
    definition.check_preparation_support()?;
    options.validate(definition)?;
    let playable = PlayableBounds {
        bounds,
        operating_revision: 1,
    };
    playable.validate(definition)?;
    mc_world::atomic_file::create_dir_all_durable(directory)?;
    let lease = acquire_exclusive_world_root_lease(directory)?;
    if directory.join(WORLD_MANIFEST).exists() {
        return PreparedWorld::open_leased(directory, Some(definition), options, lease);
    }
    // Do not convert an existing Anvil/user directory or hide previous state.
    for entry in std::fs::read_dir(directory)? {
        let name = entry?.file_name();
        if ![
            STAGING,
            ARTIFACTS,
            ".solaris-world.lock",
            "world.manifest.toml.tmp",
        ]
        .iter()
        .any(|allowed| name == *allowed)
        {
            return Err(PrepareError::Invalid(
                "prepare requires a new world directory or verified W1 orphan",
            ));
        }
    }
    let root_bytes = WorldPlan::build(definition)?.canonical_bytes()?;
    if root_bytes.len() as u64 > definition.estimate()?.root_payload_limit_bytes {
        return Err(PrepareError::Invalid(
            "root encoding exceeds admitted payload bound",
        ));
    }
    WorldPlan::from_canonical_bytes(&root_bytes, definition)?;
    let manifest = Manifest::new(definition, &root_bytes)?;
    let artifact_dir = directory.join(ARTIFACTS);
    let staging = directory.join(STAGING);
    if artifact_dir.exists() {
        if staging.exists() {
            return Err(PrepareError::Invalid(
                "both staging and prepared artifacts exist; manual recovery required",
            ));
        }
        verify_descriptor(&artifact_dir, &manifest)?;
        let (_, recovered_bounds, store) =
            load_artifacts(&artifact_dir, &manifest, definition, options)?;
        if recovered_bounds != playable {
            return Err(PrepareError::Invalid("orphan playable bounds mismatch"));
        }
        if store.record_counts()? != mc_world::native::NativeRecordCounts::default() {
            return Err(PrepareError::Invalid(
                "orphan state store is not empty; refuse publication",
            ));
        }
        drop(store);
    } else {
        std::fs::create_dir_all(&staging)?;
        sync_parent_dir(&staging)?;
        let descriptor = staging.join(DESCRIPTOR);
        if descriptor.exists() {
            verify_descriptor(&staging, &manifest)?;
        } else {
            // A crash before descriptor publication leaves no ready artifacts.
            if std::fs::read_dir(&staging)?
                .any(|entry| entry.is_ok_and(|entry| entry.file_name() != "prepared.toml.tmp"))
            {
                return Err(PrepareError::Invalid(
                    "unidentified staging artifact; manual recovery required",
                ));
            }
            write_atomic_toml(&descriptor, &manifest)?;
        }
        observed(PreparePhase::Descriptor)?;
        let root_path = staging.join(ROOT);
        if root_path.exists() {
            if read_bounded(&root_path, definition.estimate()?.root_payload_limit_bytes)?
                != root_bytes
            {
                return Err(PrepareError::Invalid("orphan root mismatch"));
            }
        } else {
            write_new_durable(&root_path, &root_bytes)?;
        }
        observed(PreparePhase::Root)?;
        let native_path = staging.join(NATIVE);
        let store = if native_path.exists() {
            NativeStore::open_with_cache(
                &native_path,
                manifest.identity.bytes(),
                options.native_cache_bytes,
            )?
        } else {
            NativeStore::create_with_cache(
                &native_path,
                manifest.identity.bytes(),
                options.native_cache_bytes,
            )?
        };
        // An unpublished prepare may never contain gameplay/imported state.
        if store.record_counts()? != mc_world::native::NativeRecordCounts::default() {
            return Err(PrepareError::Invalid(
                "orphan state store is not empty; refuse replacement",
            ));
        }
        drop(store);
        File::open(&native_path)?.sync_all()?;
        let bounds_path = staging.join(BOUNDS);
        if bounds_path.exists() {
            if read_toml::<PlayableBounds>(&bounds_path)? != playable {
                return Err(PrepareError::Invalid("orphan playable bounds mismatch"));
            }
        } else {
            write_atomic_toml(&bounds_path, &playable)?;
        }
        observed(PreparePhase::Store)?;
        let (_, _, store) = load_artifacts(&staging, &manifest, definition, options)?;
        drop(store);
        std::fs::rename(&staging, &artifact_dir)?;
        sync_parent_dir(&artifact_dir)?;
    }
    // Recovery may observe fully written bytes that never reached their fsync.
    // Re-establish every artifact barrier before publishing the commit marker.
    for name in [DESCRIPTOR, ROOT, NATIVE, BOUNDS] {
        File::open(artifact_dir.join(name))?.sync_all()?;
    }
    sync_parent_dir(&artifact_dir.join(ROOT))?;
    sync_parent_dir(&artifact_dir)?;
    observed(PreparePhase::Artifacts)?;
    write_atomic_toml(&directory.join(WORLD_MANIFEST), &manifest)?;
    observed(PreparePhase::Manifest)?;
    PreparedWorld::open_leased(directory, Some(definition), options, lease)
}

fn verify_descriptor(directory: &Path, expected: &Manifest) -> Result<(), PrepareError> {
    let found: Manifest = read_toml(&directory.join(DESCRIPTOR))?;
    if &found != expected {
        return Err(PrepareError::IdentityMismatch);
    }
    found.validate(Some(&expected.definition.clone().validate()?))?;
    Ok(())
}

fn load_artifacts(
    directory: &Path,
    manifest: &Manifest,
    definition: &WorldDefinition,
    options: PrepareOptions,
) -> Result<(WorldPlan, PlayableBounds, NativeStore), PrepareError> {
    let (root_plan, playable) = load_canonical_artifacts(directory, manifest, definition)?;
    let store = NativeStore::open_with_cache(
        &directory.join(NATIVE),
        manifest.native_store_identity.bytes(),
        options.native_cache_bytes,
    )?;
    Ok((root_plan, playable, store))
}

fn load_canonical_artifacts(
    directory: &Path,
    manifest: &Manifest,
    definition: &WorldDefinition,
) -> Result<(WorldPlan, PlayableBounds), PrepareError> {
    let root_bytes = read_bounded(
        &directory.join(ROOT),
        definition.estimate()?.root_payload_limit_bytes,
    )?;
    if root_bytes.len() as u64 != manifest.root_payload_bytes
        || hash(&root_bytes) != manifest.root_hash
    {
        return Err(PrepareError::Invalid("canonical root checksum mismatch"));
    }
    let root_plan = WorldPlan::from_canonical_bytes(&root_bytes, definition)?;
    let playable: PlayableBounds = read_toml(&directory.join(BOUNDS))?;
    playable.validate(definition)?;
    Ok((root_plan, playable))
}

/// Operator definition inputs use the same bounded decoder as publication.
pub fn read_definition_file(path: &Path) -> Result<DefinitionInput, PrepareError> {
    read_toml(path)
}

pub(super) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, PrepareError> {
    let file = File::open(path)?;
    if file.metadata()?.len() > limit {
        return Err(PrepareError::Invalid(
            "artifact length exceeds decode limit",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(PrepareError::Invalid("artifact grew beyond decode limit"));
    }
    Ok(bytes)
}
fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, PrepareError> {
    let bytes = read_bounded(path, MAX_MANIFEST_BYTES)?;
    let text =
        std::str::from_utf8(&bytes).map_err(|_| PrepareError::Invalid("manifest is not UTF-8"))?;
    Ok(toml::from_str(text)?)
}
fn write_new_durable(path: &Path, bytes: &[u8]) -> Result<(), PrepareError> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    sync_parent_dir(path)?;
    Ok(())
}
fn write_atomic_toml<T: Serialize>(path: &Path, value: &T) -> Result<(), PrepareError> {
    let temporary = path.with_file_name(format!(
        "{}.tmp",
        path.file_name().unwrap().to_string_lossy()
    ));
    // The lease excludes other writers; an interrupted temporary is never ready.
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)?;
    file.write_all(toml::to_string(value)?.as_bytes())?;
    file.sync_all()?;
    drop(file);
    replace_file_durable(&temporary, path)?;
    Ok(())
}

#[cfg(test)]
#[path = "prepared_tests.rs"]
mod tests;
