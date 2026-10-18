use std::io::{BufRead as _, Read as _, Write as _};
use std::process::{Child, Command, Stdio};

use super::{WORLD_LEASE_FILE_NAME, acquire_exclusive_world_root_lease};
use crate::storage::WorldError;

const HELPER_ROOT_ENV: &str = "SOLARIS_WORLD_LEASE_HELPER_ROOT";
const READY_MARKER: &str = "SOLARIS_WORLD_LEASE_READY";

struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    fn terminate(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[test]
fn world_lease_subprocess_helper() {
    let Some(root) = std::env::var_os(HELPER_ROOT_ENV) else {
        return;
    };
    let _lease = acquire_exclusive_world_root_lease(std::path::Path::new(&root))
        .expect("helper acquires lease");
    println!("{READY_MARKER} pid={}", std::process::id());
    std::io::stdout().flush().unwrap();

    let mut release = [0_u8; 1];
    std::io::stdin().read_exact(&mut release).unwrap();
}

#[test]
fn writable_world_lease_is_process_exclusive_and_recovers_after_crash() {
    let first_root = tempfile::tempdir().unwrap();
    let second_root = tempfile::tempdir().unwrap();

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "storage::world_lease::tests::world_lease_subprocess_helper",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HELPER_ROOT_ENV, first_root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let child_pid = child.id();
    let stdout = child.stdout.take().unwrap();
    let mut child = ChildGuard { child };
    let mut output = std::io::BufReader::new(stdout);
    let mut line = String::new();
    let mut ready = false;
    for _ in 0..32 {
        line.clear();
        if output.read_line(&mut line).unwrap() == 0 {
            break;
        }
        if line.contains(READY_MARKER) {
            ready = true;
            break;
        }
    }
    assert!(ready, "lease helper exited before publishing readiness");

    let error = match acquire_exclusive_world_root_lease(first_root.path()) {
        Ok(_) => panic!("second process unexpectedly acquired writable world lease"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        WorldError::WorldLocked { metadata, .. }
            if metadata.contains(&format!("pid={child_pid}"))
                && metadata.contains("started_unix_ms=")
                && metadata.contains("instance=")
    ));

    let distinct = acquire_exclusive_world_root_lease(second_root.path()).unwrap();
    drop(distinct);

    let lease_path = first_root.path().join(WORLD_LEASE_FILE_NAME);
    let stale_metadata = std::fs::read_to_string(&lease_path).unwrap();
    assert!(stale_metadata.contains(&format!("pid={child_pid}")));

    child.terminate();

    let reopened = acquire_exclusive_world_root_lease(first_root.path()).unwrap();
    let refreshed_metadata = std::fs::read_to_string(&lease_path).unwrap();
    assert!(refreshed_metadata.contains(&format!("pid={}", std::process::id())));
    assert_ne!(refreshed_metadata, stale_metadata);
    drop(reopened);
}

#[test]
fn world_reopen_releases_lease_with_inherited_file_description() {
    let root = tempfile::tempdir().unwrap();
    let lease = acquire_exclusive_world_root_lease(root.path()).unwrap();
    // dup and fork share the same open file description on Unix. A concurrent
    // process spawn may retain it until exec, despite close-on-exec on our fd.
    let inherited_file = lease._file.try_clone().unwrap();
    drop(lease);
    let _reopened = acquire_exclusive_world_root_lease(root.path()).unwrap();
    drop(inherited_file);
}

#[test]
fn world_lease_excludes_other_owners_until_release() {
    let root = tempfile::tempdir().unwrap();
    let lease = acquire_exclusive_world_root_lease(root.path()).unwrap();
    assert!(matches!(
        acquire_exclusive_world_root_lease(root.path()),
        Err(WorldError::WorldLocked { .. })
    ));
    drop(lease);
    let _reopened = acquire_exclusive_world_root_lease(root.path()).unwrap();
}
