//! Deterministic identity for server-spawned entities.
//!
//! A UUID is a keyed hash of `(spawn tick, wire id)` under a per-world random
//! salt. Wire ids are never handed out twice within one run and the tick clock
//! is recovered monotonically across restarts, so the pair is unique without
//! any new persisted counter, while two worlds that share a salt (a checkpoint
//! copy, i.e. a replay) and are fed the same inputs mint the same ids.
//!
//! The salt is secret: clients see every UUID, the wire id and the world
//! tick, so anything derivable from public values (the world seed, an
//! invertible mixer) would let a client recover the seed. SHA-256 under a
//! secret prefix is one-way for the salt and the seed is not an input at all.
//! The salt is minted once per world and persisted next to the terrain epoch
//! in `level.dat/Data` (`SolarisEntitySalt`), so it rides every checkpoint.
use sha2::{Digest, Sha256};

use crate::boundary::EntityId;
use crate::config::CoreError;
use crate::tick::TickState;

/// Per-world salt width in bytes.
pub(crate) const SALT_LEN: usize = 16;
pub(crate) const SALT_FIELD: &str = "SolarisEntitySalt";

/// UUID (v4 layout) for the entity `id` spawned at `tick` under `salt`.
pub(crate) fn derive(salt: &[u8; SALT_LEN], tick: u64, id: EntityId) -> uuid::Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"solaris:entity-uuid:v1");
    hasher.update(salt);
    hasher.update(tick.to_be_bytes());
    hasher.update(id.0.to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_random_bytes(bytes).into_uuid()
}

impl TickState {
    /// UUID for an entity being spawned now with wire id `id`.
    pub(crate) fn entity_uuid(&self, id: EntityId) -> uuid::Uuid {
        derive(&self.entity_salt, self.tick, id)
    }
}

/// Load the world's salt from the staged native globals (minting and staging
/// it when the world has none yet) and install it in the root world and every
/// dimension. A fresh world's initial checkpoint persists it before any tick;
/// a reopened pre-salt world persists it at its next checkpoint, so a crash
/// before then remints it (later uuids stay unique, only replay of that
/// first window diverges).
pub(crate) fn install_salt(root: &mut TickState) -> Result<(), CoreError> {
    let stored = crate::terrain::read_data_field(&root.native_globals, SALT_FIELD)
        .map_err(|error| CoreError::Storage(format!("entity salt: {error}")))?;
    let salt = match stored {
        Some(text) => uuid::Uuid::try_parse(&text).map_err(|error| {
            CoreError::Storage(format!("entity salt {text:?} is not a uuid: {error}"))
        })?,
        None => {
            let salt = uuid::Uuid::new_v4();
            crate::terrain::stage_data_field(
                &mut root.native_globals,
                SALT_FIELD,
                &salt.simple().to_string(),
            )?;
            salt
        }
    };
    let salt = salt.into_bytes();
    root.entity_salt = salt;
    for child in root.other_dimensions.values_mut() {
        child.entity_salt = salt;
    }
    Ok(())
}
