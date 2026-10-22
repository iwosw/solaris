//! Hot native backup: a consistent copy of a running world.
//!
//! The native writer thread is the only redb writer and runs jobs FIFO, so while
//! it executes [`copy_world`] the database file is exactly the last committed
//! checkpoint (an Immediate commit is on disk when `commit` returns; no
//! transaction is open). Published WAL batches are immutable and only the
//! writer thread prunes them, so the contiguous batches after that checkpoint
//! are copied up to the tick the journal thread had durably appended when it
//! accepted the request. Recovery from the copy therefore reaches that tick.
//! Reads continue on every other thread; ticks and WAL appends are not paused.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::config::CoreError;
use crate::journal::parse_batch_name;

/// Dirty-page burst limit while copying: sync the output this often so the
/// concurrent WAL fsyncs never queue behind hundreds of MB of dirty pages.
const SYNC_EVERY_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) const NATIVE_DATABASE: &str = "native/state.redb";

/// What a completed hot backup contains. `files` maps world-relative names to
/// lowercase hex SHA-256 digests of the copied bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotBackup {
    /// Tick of the checkpoint stored in the copied database.
    pub checkpoint_tick: u64,
    /// Last tick recoverable from the copy: checkpoint plus copied WAL.
    pub wal_tick: u64,
    pub files: BTreeMap<String, String>,
}

fn io(error: std::io::Error) -> CoreError {
    CoreError::Storage(error.to_string())
}

fn copy_hashed(source: &Path, destination: &Path) -> Result<String, CoreError> {
    let mut input = File::open(source)
        .map_err(|error| CoreError::Storage(format!("{}: {error}", source.display())))?;
    let mut output = File::options()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| CoreError::Storage(format!("{}: {error}", destination.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 256 * 1024];
    let mut unsynced = 0;
    loop {
        let count = input.read(&mut buffer).map_err(io)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count]).map_err(io)?;
        hasher.update(&buffer[..count]);
        unsynced += count as u64;
        if unsynced >= SYNC_EVERY_BYTES {
            output.sync_data().map_err(io)?;
            unsynced = 0;
        }
    }
    output.sync_all().map_err(io)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Copy the checkpoint database at `checkpoint_tick` and the contiguous WAL
/// batches `(checkpoint_tick, through]` into the existing empty directory `out`.
/// Must run on the native writer thread between jobs.
pub(crate) fn copy_world(
    world_dir: &Path,
    out: &Path,
    checkpoint_tick: u64,
    through: u64,
) -> Result<HotBackup, CoreError> {
    let through = through.max(checkpoint_tick);
    let mut files = BTreeMap::new();
    std::fs::create_dir(out.join("native")).map_err(io)?;
    std::fs::create_dir(out.join("journal")).map_err(io)?;
    files.insert(
        NATIVE_DATABASE.to_owned(),
        copy_hashed(&world_dir.join(NATIVE_DATABASE), &out.join(NATIVE_DATABASE))?,
    );

    let mut batches = Vec::new();
    for entry in std::fs::read_dir(world_dir.join("journal")).map_err(io)? {
        let name = entry
            .map_err(io)?
            .file_name()
            .to_string_lossy()
            .into_owned();
        if let Some((lo, hi)) = parse_batch_name(&name)
            && hi > checkpoint_tick
            && lo <= through
        {
            batches.push((lo, hi, name));
        }
    }
    batches.sort();
    let mut covered = checkpoint_tick;
    for (lo, hi, name) in &batches {
        if *lo != covered + 1 {
            return Err(CoreError::Storage(format!(
                "hot backup: journal batch {name} starts at {lo}, expected {}",
                covered + 1
            )));
        }
        let relative = format!("journal/{name}");
        let digest = copy_hashed(&world_dir.join(&relative), &out.join(&relative))?;
        files.insert(relative, digest);
        covered = *hi;
    }
    if covered < through {
        return Err(CoreError::Storage(format!(
            "hot backup: journal hole: ticks {}..={through} have no batch",
            covered + 1
        )));
    }
    for directory in [out.join("native"), out.join("journal"), out.to_path_buf()] {
        File::open(directory)
            .and_then(|d| d.sync_all())
            .map_err(io)?;
    }
    Ok(HotBackup {
        checkpoint_tick,
        wal_tick: covered,
        files,
    })
}
