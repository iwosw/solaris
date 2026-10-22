//! Offline native-v2 world backups: one exclusive directory lease, the native
//! database's own lock, and a SHA-256 manifest covering the store, WAL and metadata.
//! Restore validates the entire backup before creating or writing target data.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

const BACKUP_MANIFEST: &str = "manifest.json";
const NATIVE_DATABASE: &str = "native/state.redb";
const BACKUP_FORMAT: u32 = 2;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct BackupManifest {
    format: u32,
    solaris_version: String,
    target_release: String,
    protocol_version: i32,
    world_version: u32,
    world_contract_schema: u32,
    worldgen_revision: u32,
    native_prototype_schema: u32,
    files: BTreeMap<String, String>,
}

impl BackupManifest {
    fn current() -> Self {
        Self {
            format: BACKUP_FORMAT,
            solaris_version: env!("CARGO_PKG_VERSION").to_owned(),
            target_release: mc_protocol::TARGET_RELEASE.to_owned(),
            protocol_version: mc_protocol::PROTOCOL_VERSION,
            world_version: mc_protocol::WORLD_VERSION,
            world_contract_schema: mc_worldgen::world_contract::WORLD_CONTRACT_SCHEMA,
            worldgen_revision: mc_worldgen::WORLDGEN_REVISION,
            native_prototype_schema: mc_world::native::NATIVE_PROTOTYPE_SCHEMA,
            files: BTreeMap::new(),
        }
    }
}

/// Core::open locks the directory inode, not the v1 .solaris-world.lock file.
/// redb 3 uses the same File::try_lock on state.redb. Locking the raw file also
/// excludes direct NativeStore users without opening/repairing the database.
struct ExclusiveLease(File);

impl ExclusiveLease {
    fn acquire(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        file.try_lock().map_err(|error| {
            anyhow::anyhow!(
                "{} is already leased or cannot be locked: {error}",
                path.display()
            )
        })?;
        Ok(Self(file))
    }
}

impl Drop for ExclusiveLease {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn is_lease_file(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".solaris-world.lock" | "session.lock")
    )
}

fn validate_name(name: &str) -> Result<()> {
    // Reject noncanonical names too: aliases such as a//b or a/./b must not
    // resolve to a second manifest entry for the same destination.
    if name.is_empty()
        || name == BACKUP_MANIFEST
        || name.contains(['\\', '\0'])
        || name.split('/').any(|part| matches!(part, "" | "." | ".."))
        || Path::new(name)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || is_lease_file(Path::new(name))
    {
        bail!("backup manifest lists an unsafe path: {name:?}");
    }
    Ok(())
}

fn collect_files(root: &Path, backup: bool) -> Result<BTreeMap<String, PathBuf>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(&directory)
            .with_context(|| format!("reading {}", directory.display()))?
        {
            let path = entry?.path();
            // DirEntry::file_type does not follow links, including directory
            // links. Never copy bytes from outside the world/backup root.
            let kind = std::fs::symlink_metadata(&path)?.file_type();
            if kind.is_symlink() || (!kind.is_dir() && !kind.is_file()) {
                bail!(
                    "backup refuses symlink or non-regular file: {}",
                    path.display()
                );
            }
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            if is_lease_file(&path) {
                continue;
            }
            let name = path
                .strip_prefix(root)?
                .to_str()
                .with_context(|| format!("non-UTF-8 file name under {}", root.display()))?
                .to_owned();
            if backup && name == BACKUP_MANIFEST {
                continue;
            }
            validate_name(&name)?;
            files.insert(name, path);
        }
    }
    Ok(files)
}

/// Canonicalize existing ancestors without creating a destination merely to
/// check overlap. In particular, a corrupt backup leaves an absent target absent.
fn resolve_destination(path: &Path) -> Result<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A dangling symlink is not an absent destination.
            if std::fs::symlink_metadata(path).is_ok() {
                bail!("destination is a dangling symlink: {}", path.display());
            }
            let absolute = std::path::absolute(path)?;
            let name = absolute
                .file_name()
                .context("destination has no file name")?;
            let parent = absolute.parent().context("destination has no parent")?;
            Ok(resolve_destination(parent)?.join(name))
        }
        Err(error) => Err(error).with_context(|| format!("resolving {}", path.display())),
    }
}

fn refuse_overlap(source: &Path, destination: &Path) -> Result<()> {
    if source.starts_with(destination) || destination.starts_with(source) {
        bail!(
            "destination {} overlaps source {}; use a separate directory",
            destination.display(),
            source.display(),
        );
    }
    Ok(())
}

fn require_empty(path: &Path) -> Result<()> {
    match std::fs::read_dir(path) {
        Ok(mut entries) => {
            if entries.next().transpose()?.is_some() {
                bail!("destination is not empty: {}", path.display());
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn digest_file(path: &Path) -> Result<String> {
    let mut source = File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Copy without replacing any existing file. Hash the bytes actually copied,
/// not a separately opened view of a potentially different source file.
fn copy_file_durable(source: &Path, destination: &Path) -> Result<String> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut input = File::open(source)?;
    let mut output = File::options()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        hasher.update(&buffer[..count]);
    }
    output.sync_all()?;
    mc_world::atomic_file::sync_parent_dir(destination)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Copy a stopped native-v2 world, including its WAL and world metadata.
/// Neither an active Core nor a separately opened native store may own it.
pub fn run_backup(world_dir: &Path, out: &Path) -> Result<()> {
    let world = std::fs::canonicalize(world_dir)
        .with_context(|| format!("resolving world {}", world_dir.display()))?;
    if !world.is_dir() {
        bail!("backup source is not a directory: {}", world.display());
    }
    let out = resolve_destination(out)?;
    refuse_overlap(&world, &out)?;
    let _world_lease = ExclusiveLease::acquire(&world)?;
    let files = collect_files(&world, false)?;
    if !files.contains_key(NATIVE_DATABASE) {
        bail!("backup requires a native-v2 world with {NATIVE_DATABASE}");
    }
    let _database_lease = ExclusiveLease::acquire(&world.join(NATIVE_DATABASE))?;
    require_empty(&out)?;
    std::fs::create_dir_all(&out)?;
    let _output_lease = ExclusiveLease::acquire(&out)?;
    require_empty(&out)?;
    let mut manifest = BackupManifest::current();
    for (name, source) in files {
        let digest = copy_file_durable(&source, &out.join(&name))
            .with_context(|| format!("backing up {name}"))?;
        manifest.files.insert(name, digest);
    }
    write_manifest(&out, &manifest)?;
    println!(
        "backup of {} into {} complete: {} files",
        world.display(),
        out.display(),
        manifest.files.len(),
    );
    Ok(())
}

/// Create the manifest last: a directory without it is never a valid backup.
fn write_manifest(out: &Path, manifest: &BackupManifest) -> Result<()> {
    let path = out.join(BACKUP_MANIFEST);
    let mut file = File::options().write(true).create_new(true).open(&path)?;
    file.write_all(&serde_json::to_vec_pretty(manifest)?)?;
    file.sync_all()?;
    mc_world::atomic_file::sync_parent_dir(&path)?;
    mc_world::atomic_file::sync_parent_dir(out)?;
    Ok(())
}

/// `YYYYMMDDTHHMMSSZ` for Unix seconds (proleptic Gregorian, civil-from-days).
fn utc_stamp(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64 + 719_468;
    let seconds = unix_seconds % 86_400;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

/// Hot backup of a running core into `backup_dir/backup-<UTC>-tick<N>/`, the
/// same manifest format `run_restore` verifies. The copy is built in a hidden
/// `.partial` directory and renamed only once the manifest is durable, so an
/// interrupted or failed backup never looks like a valid one.
pub fn run_hot_backup(
    core: &mc_core::Core,
    backup_dir: &Path,
) -> Result<(PathBuf, mc_core::hot_backup::HotBackup)> {
    std::fs::create_dir_all(backup_dir)
        .with_context(|| format!("creating {}", backup_dir.display()))?;
    let backup_dir = std::fs::canonicalize(backup_dir)?;
    let stamp = utc_stamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs(),
    );
    let partial = backup_dir.join(format!(".backup-{stamp}.partial"));
    std::fs::create_dir(&partial).with_context(|| format!("creating {}", partial.display()))?;
    let result = (|| {
        let backup = core.backup(&partial)?;
        let mut manifest = BackupManifest::current();
        manifest.files.clone_from(&backup.files);
        write_manifest(&partial, &manifest)?;
        let finished = backup_dir.join(format!("backup-{stamp}-tick{}", backup.wal_tick));
        std::fs::rename(&partial, &finished)
            .with_context(|| format!("publishing {}", finished.display()))?;
        mc_world::atomic_file::sync_parent_dir(&finished)?;
        Ok((finished, backup))
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&partial);
    }
    result
}

fn read_manifest(from: &Path) -> Result<BackupManifest> {
    let path = from.join(BACKUP_MANIFEST);
    if !std::fs::symlink_metadata(&path)?.file_type().is_file() {
        bail!("backup manifest is not a regular file: {}", path.display());
    }
    let manifest: BackupManifest = serde_json::from_slice(&std::fs::read(&path)?)
        .with_context(|| format!("parsing {}", path.display()))?;
    let current = BackupManifest::current();
    if manifest.format != BACKUP_FORMAT {
        bail!(
            "unsupported native backup format {}; expected {BACKUP_FORMAT}",
            manifest.format
        );
    }
    if manifest.target_release != current.target_release
        || manifest.protocol_version != current.protocol_version
        || manifest.world_version != current.world_version
        || manifest.world_contract_schema != current.world_contract_schema
        || manifest.worldgen_revision != current.worldgen_revision
        || manifest.native_prototype_schema != current.native_prototype_schema
    {
        bail!("backup release or native world schema does not match this binary; restore refused");
    }
    if !manifest.files.contains_key(NATIVE_DATABASE) {
        bail!("backup manifest has no {NATIVE_DATABASE}; restore refused");
    }
    for name in manifest.files.keys() {
        validate_name(name)?;
    }
    Ok(manifest)
}

/// Verify every manifest path and hash before writing any world data. Restore
/// never merges into a nonempty target, follows links, or overlaps its source.
pub fn run_restore(from: &Path, world_dir: &Path) -> Result<()> {
    let from = std::fs::canonicalize(from)
        .with_context(|| format!("resolving backup {}", from.display()))?;
    let target = resolve_destination(world_dir)?;
    refuse_overlap(&from, &target)?;
    let _source_lease = ExclusiveLease::acquire(&from)?;
    let manifest = read_manifest(&from)?;
    let files = collect_files(&from, true)?;
    if !files.keys().eq(manifest.files.keys()) {
        bail!("backup files do not match manifest; restore refused");
    }
    let _database_lease = ExclusiveLease::acquire(&from.join(NATIVE_DATABASE))?;
    // Lease an existing target before inspecting it; a live core must fail on
    // its actual lock even when it has not yet published any files.
    let target_lease = if target.try_exists()? {
        Some(ExclusiveLease::acquire(&target)?)
    } else {
        None
    };
    require_empty(&target)?;
    for (name, source) in &files {
        let actual = digest_file(source)?;
        let expected = &manifest.files[name];
        if &actual != expected {
            bail!(
                "backup file {name} failed verification: expected {expected}, actual {actual}; nothing was restored"
            );
        }
    }
    std::fs::create_dir_all(&target)?;
    let _target_lease = match target_lease {
        Some(lease) => lease,
        None => ExclusiveLease::acquire(&target)?,
    };
    require_empty(&target)?;
    for (name, source) in &files {
        let actual = copy_file_durable(source, &target.join(name))
            .with_context(|| format!("restoring {name}"))?;
        if actual != manifest.files[name] {
            bail!("backup file {name} changed while restoring; restore is incomplete");
        }
    }
    mc_world::atomic_file::sync_parent_dir(&target)?;
    println!(
        "restore of {} into {} complete: {} files verified",
        from.display(),
        target.display(),
        files.len(),
    );
    Ok(())
}

#[cfg(test)]
#[path = "backup_tests.rs"]
mod tests;
