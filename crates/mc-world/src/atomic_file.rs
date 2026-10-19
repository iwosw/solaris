//! Cross-platform same-directory atomic file replacement and durability helpers.

use std::io;
use std::path::Path;

/// Atomically install `temporary` as `target`, replacing an existing target.
///
/// The temporary file must already be fully written and synced by the caller.
/// On replacement failure the temporary path is removed best-effort.
pub fn replace_file(temporary: &Path, target: &Path) -> io::Result<()> {
    // `rename` replaces an existing target on Unix and, via MoveFileExW with
    // MOVEFILE_REPLACE_EXISTING, on Windows.
    let result = std::fs::rename(temporary, target);
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

/// Replace the target and then persist the containing directory entry where the
/// platform supports explicit directory synchronization.
pub fn replace_file_durable(temporary: &Path, target: &Path) -> io::Result<()> {
    replace_file(temporary, target)?;
    sync_parent_dir(target)
}

/// Persist the parent directory entry after create/rename.
pub fn sync_parent_dir(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    #[cfg(unix)]
    {
        std::fs::File::open(parent)?.sync_all()
    }
    #[cfg(windows)]
    {
        // Windows cannot open directories as files; there is no directory fsync.
        let _ = parent;
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = parent;
        Ok(())
    }
}

/// Persist every directory entry created by `create_dir_all`, including a
/// previously unflushed directory left by an interrupted attempt.
pub fn create_dir_all_durable(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    for directory in path.ancestors() {
        if directory.as_os_str().is_empty() || directory.parent().is_none() {
            break;
        }
        sync_parent_dir(directory)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_missing_and_existing_targets() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("state.dat");
        let first = root.path().join("state.dat.tmp.1");
        std::fs::write(&first, b"first").unwrap();
        replace_file_durable(&first, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"first");
        assert!(!first.exists());

        let second = root.path().join("state.dat.tmp.2");
        std::fs::write(&second, b"second").unwrap();
        replace_file_durable(&second, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"second");
        assert!(!second.exists());
    }

    #[test]
    fn replacement_failure_cleans_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let temporary = root.path().join("state.tmp");
        let target = root.path().join("missing-parent/state.dat");
        std::fs::write(&temporary, b"payload").unwrap();

        assert!(replace_file(&temporary, &target).is_err());
        assert!(!temporary.exists());
        assert!(!target.exists());
    }
}
