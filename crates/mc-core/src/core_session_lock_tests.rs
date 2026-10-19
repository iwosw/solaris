//! Live native session-lock projection: `Core::session_locked` mirrors the
//! tick thread's authoritative lock set so the edge can fence edge-side
//! effects before the native gate runs.

use crate::boundary::{ConnectionId, PlayerInput};
use crate::config::CoreConfig;
use crate::core::Core;

fn test_config(dir: std::path::PathBuf, require_plugin_login: bool) -> CoreConfig {
    let mut config = CoreConfig::new(dir, 42);
    config.view_distance = 1;
    config.simulation_distance = 0;
    config.checkpoint_every_ticks = u64::MAX;
    config.require_plugin_login = require_plugin_login;
    config
}

fn open_core(require_plugin_login: bool) -> (Core, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mc-core-session-lock-{}-{}-{:?}",
        require_plugin_login,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let core = Core::open(test_config(dir.clone(), require_plugin_login)).expect("open");
    (core, dir)
}

fn join(connection: ConnectionId) -> PlayerInput {
    PlayerInput::Join {
        connection,
        uuid: uuid::Uuid::from_u128(u128::from(connection.0)),
        name: format!("probe{}", connection.0),
        permission_level: 4,
    }
}

fn wait_ticks(core: &Core, ticks: usize) {
    let mut rx = core.subscribe();
    for _ in 0..ticks {
        match rx.blocking_recv() {
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                panic!("tick thread died")
            }
        }
        let _ = core.try_drain_outbox();
    }
}

#[test]
fn session_lock_mirror_tracks_join_and_starts_unlocked_when_off() {
    let (core, dir) = open_core(false);
    assert!(!core.session_locked(ConnectionId(11)));
    core.submit(join(ConnectionId(11)));
    wait_ticks(&core, 3);
    assert!(
        !core.session_locked(ConnectionId(11)),
        "config-off sessions never lock"
    );
    core.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn session_lock_mirror_reports_locked_join_when_required() {
    let (core, dir) = open_core(true);
    assert!(
        core.session_locked(ConnectionId(13)),
        "unknown sessions fail closed before any tick"
    );
    core.submit(join(ConnectionId(13)));
    wait_ticks(&core, 3);
    assert!(
        core.session_locked(ConnectionId(13)),
        "required join must project locked"
    );
    core.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
