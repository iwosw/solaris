//! Real guest configure -> command -> native mob -> checkpoint -> restart.
use crate::boundary::{ConnectionId, PlayerInput, TickOutput};
use crate::config::CoreConfig;
use crate::core::Core;

#[path = "../../mc-plugin-host/tests/fixture/mod.rs"]
pub(super) mod fixture;

fn next(core: &Core, rx: &mut tokio::sync::broadcast::Receiver<TickOutput>) -> TickOutput {
    loop {
        let result = rx.blocking_recv();
        core.try_drain_outbox();
        match result {
            Ok(output) => return output,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(error) => panic!("native tick stopped: {error}"),
        }
    }
}

#[test]
fn guest_sky_creeper_spawns_flies_and_survives_checkpoint_restart() {
    let directory = tempfile::tempdir().unwrap();
    let packages = directory.path().join("plugins");
    let package = packages.join("sky-creeper");
    std::fs::create_dir_all(&package).unwrap();
    let sdk = fixture::repo_root().join("sdk/rust");
    let bytes = fixture::component_bytes_from_workspace(
        &sdk,
        "solaris-sky-creeper-plugin",
        "solaris_sky_creeper_plugin.wasm",
    );
    std::fs::write(package.join("plugin.wasm"), bytes).unwrap();
    std::fs::write(
        package.join("plugin.toml"),
        include_str!("../../../sdk/rust/examples/sky-creeper/plugin.toml"),
    )
    .unwrap();
    let mut config = CoreConfig::new(directory.path().join("world"), 42);
    config.plugin_directory = Some(packages);
    config.view_distance = 2;
    config.simulation_distance = 2;
    config.player_outbox_cap = 8192;
    config.checkpoint_every_ticks = u64::MAX;
    let core = Core::open(config.clone()).unwrap();
    let mut rx = core.subscribe();
    let player_uuid = uuid::Uuid::from_u128(0x534b595f43524545504552);
    core.submit(PlayerInput::Join {
        connection: ConnectionId(1),
        uuid: player_uuid,
        name: "SkyProbe".into(),
        permission_level: 4,
    });
    let mut player_position = None;
    for _ in 0..100 {
        let output = next(&core, &mut rx);
        if let Some(player) = output
            .snapshot
            .entities
            .iter()
            .find(|entity| entity.uuid == player_uuid)
        {
            player_position = Some(player.position);
            break;
        }
    }
    let player_position = player_position.expect("player joins real core");
    core.submit(PlayerInput::Command {
        connection: ConnectionId(1),
        command: "sky-creeper".into(),
        permission_level: 4,
    });
    let mut spawned = None;
    for _ in 0..100 {
        let output = next(&core, &mut rx);
        if let Some(entity) = output.snapshot.entities.iter().find(|entity| {
            entity.uuid != player_uuid
                && entity.position[1] > player_position[1] + 4.0
                && (entity.position[0] - player_position[0] - 8.0).abs() < 2.0
                && (entity.position[2] - player_position[2]).abs() < 2.0
        }) {
            spawned = Some(entity.clone());
            break;
        }
    }
    let spawned = spawned.expect("guest command spawns native sky creeper");
    let mut moved = spawned.clone();
    for _ in 0..5 {
        let output = next(&core, &mut rx);
        moved = output
            .snapshot
            .entities
            .iter()
            .find(|entity| entity.uuid == spawned.uuid)
            .unwrap()
            .clone();
    }
    assert!(
        moved.position[1] > player_position[1] + 3.0,
        "flyer stays aloft"
    );
    assert!(
        moved.position[0] < spawned.position[0],
        "flyer chases player"
    );
    core.shutdown(); // Shutdown commits a native checkpoint, not just a journal image.

    {
        let recovered = crate::native_checkpoint::recover_offline(&config).unwrap();
        let key = recovered
            .registry
            .entity_type_id("sky-creeper:sky_creeper")
            .unwrap();
        assert_ne!(key & crate::blueprint::CUSTOM_TYPE_FLAG, 0);
        assert_eq!(crate::blueprint::wire_type_id(key), spawned.type_id);
        let record = recovered
            .dimensions
            .values()
            .flat_map(|world| world.entities.values())
            .find(|entity| entity.uuid == spawned.uuid.to_string())
            .expect("custom UUID persisted");
        assert_eq!(
            record.type_id, key,
            "native type survives capture and journal decoding"
        );
    }
    let restarted = Core::open(config.clone()).unwrap();
    let mut rx = restarted.subscribe();
    let mut restored = false;
    for _ in 0..100 {
        let output = next(&restarted, &mut rx);
        restored |= output
            .snapshot
            .entities
            .iter()
            .any(|entity| entity.uuid == spawned.uuid && entity.type_id == spawned.type_id);
        if restored {
            break;
        }
    }
    restarted.shutdown();
    assert!(
        restored,
        "checkpoint entity appears after actual Core restart"
    );
    config.plugin_directory = None;
    assert!(
        Core::open(config.clone()).is_err(),
        "removing the defining plugin refuses world admission"
    );
    std::fs::write(
        package.join("plugin.toml"),
        include_str!("../../../sdk/rust/examples/sky-creeper/plugin.toml")
            .replace("id = \"sky-creeper\"", "id = \"foreign\""),
    )
    .unwrap();
    config.world_dir = directory.path().join("foreign-world");
    config.plugin_directory = Some(directory.path().join("plugins"));
    let error = Core::open(config).unwrap_err().to_string();
    assert!(
        error.contains("plugin foreign cannot define mob sky-creeper:sky_creeper"),
        "{error}"
    );
}

/// Deploying an auth-capable package is what turns the native login fence on:
/// with `require_plugin_login` left at its default a joined session must still
/// start locked, so `/op` and chat are refused until the package unlocks it.
#[test]
fn deploying_an_auth_capable_package_locks_new_sessions_without_the_flag() {
    let directory = tempfile::tempdir().unwrap();
    let packages = directory.path().join("plugins");
    let package = packages.join("sky-creeper");
    std::fs::create_dir_all(&package).unwrap();
    let sdk = fixture::repo_root().join("sdk/rust");
    let bytes = fixture::component_bytes_from_workspace(
        &sdk,
        "solaris-sky-creeper-plugin",
        "solaris_sky_creeper_plugin.wasm",
    );
    std::fs::write(package.join("plugin.wasm"), bytes).unwrap();
    let manifest = include_str!("../../../sdk/rust/examples/sky-creeper/plugin.toml");
    std::fs::write(
        package.join("plugin.toml"),
        format!("{manifest}capabilities = [\"auth\"]\n"),
    )
    .unwrap();
    let mut config = CoreConfig::new(directory.path().join("world"), 42);
    config.plugin_directory = Some(packages);
    config.view_distance = 2;
    config.simulation_distance = 2;
    config.checkpoint_every_ticks = u64::MAX;
    assert!(!config.require_plugin_login, "the flag stays at its default");
    let core = Core::open(config).unwrap();
    let mut rx = core.subscribe();
    core.submit(PlayerInput::Join {
        connection: ConnectionId(1),
        uuid: uuid::Uuid::from_u128(0xB055),
        name: "Boss".into(),
        permission_level: 4,
    });
    for _ in 0..5 {
        next(&core, &mut rx);
    }
    assert!(
        core.session_locked(ConnectionId(1)),
        "an unauthenticated session must be locked once an auth package is deployed"
    );
    core.shutdown();
}
