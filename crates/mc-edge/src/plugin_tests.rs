//! Real deployed components only: the ignored gate requires the first-party sibling.
//! Run `cargo test -p mc-edge --lib plugin_tests::real_sibling_economy_command_wal_and_checkpoint_are_exactly_once -- --exact --ignored --nocapture`.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use mc_core::boundary::{ClickMode, ConnectionId, PlayerChange, PlayerInput, TickOutput};
use mc_core::{Core, CoreConfig, CoreError};
use mc_data::{Identifier, ItemStack};
use tokio::sync::broadcast;

const CONNECTION: ConnectionId = ConnectionId(41);
const PLAYER: uuid::Uuid = uuid::Uuid::from_u128(0x504c_5547_494e_5632);
const PURCHASE: &str = "v2|apples,minecraft:apple,2,3,minecraft:emerald,1";
const CHILD_WORLD: &str = "SOLARIS_PLUGIN_V2_CRASH_WORLD";
const CHILD_DEPLOY: &str = "SOLARIS_PLUGIN_V2_CRASH_DEPLOY";
const READY: &str = "PLUGIN_V2_DURABLE_PURCHASE_READY";
const DEADLINE: Duration = Duration::from_secs(90);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("solaris-plugin-v2-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("create isolated acceptance directory");
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct RunningCore(Core);

impl Drop for RunningCore {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

struct CrashWriter(Child);

impl Drop for CrashWriter {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn configuration(world: &Path, deployment: &Path) -> CoreConfig {
    let mut config = CoreConfig::new(world.to_owned(), 81);
    config.vanilla_data_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
    config.plugin_directory = Some(deployment.to_owned());
    config.view_distance = 1;
    config.simulation_distance = 1;
    config.checkpoint_every_ticks = u64::MAX;
    config.journal_fsync_every_batches = 1;
    config.broadcast_cap = 256;
    config
}

fn real_economy_package() -> PathBuf {
    let root = std::env::var_os("SOLARIS_DEFAULT_PLUGINS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../solaris-default-plugins")
        });
    let package = root.join("basic-economy");
    for file in ["plugin.toml", "config.toml", "plugin.wasm"] {
        assert!(
            package.join(file).is_file(),
            "missing integration prerequisite: {}; deploy the real first-party basic-economy component or set SOLARIS_DEFAULT_PLUGINS_ROOT; no substitute guest is allowed",
            package.join(file).display()
        );
    }
    package
        .canonicalize()
        .expect("resolve deployed sibling package")
}

fn deploy_economy(source: &Path, destination: &Path) {
    let package = destination.join("basic-economy");
    std::fs::create_dir_all(&package).expect("create isolated deployment");
    for file in ["plugin.toml", "config.toml", "plugin.wasm"] {
        std::fs::copy(source.join(file), package.join(file))
            .expect("copy actual deployed first-party package bytes");
    }
    // Configure the copied real package's market away from the native spawn.
    // Otherwise a zone event can open the shop before the command, obscuring
    // which real guest entrypoint produced the host operations under test.
    // WASM, manifest, currency and purchase/refund catalog remain unchanged.
    let config_path = package.join("config.toml");
    let mut config: toml::Value = std::fs::read_to_string(&config_path)
        .unwrap()
        .parse()
        .unwrap();
    let zone = config
        .get_mut("zone")
        .and_then(toml::Value::as_table_mut)
        .expect("real basic-economy configuration has a market zone");
    for (name, x, z) in [
        ("minimum", 10_000_i64, 10_000_i64),
        ("maximum", 10_016, 10_016),
    ] {
        let position = zone
            .get_mut(name)
            .and_then(toml::Value::as_table_mut)
            .expect("real market zone has inclusive bounds");
        position.insert("x".into(), toml::Value::Integer(x));
        position.insert("z".into(), toml::Value::Integer(z));
    }
    std::fs::write(config_path, toml::to_string(&config).unwrap()).unwrap();
}

fn expect_plugin_startup_failure(config: CoreConfig) {
    match Core::open(config) {
        Err(CoreError::Plugin(error)) => assert!(!error.is_empty(), "startup must explain refusal"),
        Err(error) => panic!("invalid package failed in the wrong authority: {error}"),
        Ok(core) => {
            core.shutdown();
            panic!("invalid package was accepted instead of failing closed");
        }
    }
}

#[test]
fn malformed_plugin_manifest_fails_closed_at_core_open() {
    let directory = TestDirectory::new();
    let deployment = directory.0.join("plugins");
    let package = deployment.join("invalid-package");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("plugin.toml"), "id = [not valid TOML\n").unwrap();
    let world = directory.0.join("world");
    expect_plugin_startup_failure(configuration(&world, &deployment));
    assert!(!world.join("native/state.redb").exists());
}

#[test]
fn declared_precommit_hooks_fail_before_component_compilation_and_world_admission() {
    let directory = TestDirectory::new();
    let deployment = directory.0.join("plugins");
    let package = deployment.join("hook-owner");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("plugin.toml"),
        "id = 'hook-owner'\nname = 'Hook owner'\nversion = '1'\napi = '0.7.0'\nhooks = ['before-damage']\n",
    )
    .unwrap();
    std::fs::write(package.join("plugin.wasm"), b"not a component").unwrap();
    let world = directory.0.join("world");
    match Core::open(configuration(&world, &deployment)) {
        Err(CoreError::Plugin(error)) => {
            assert!(error.contains("hook-owner"), "{error}");
            assert!(error.contains("before-damage"), "{error}");
            assert!(error.contains("does not support"), "{error}");
        }
        Err(error) => panic!("hook declaration failed in the wrong authority: {error}"),
        Ok(core) => {
            core.shutdown();
            panic!("unsupported precommit hook was silently accepted");
        }
    }
    assert!(!world.join("native/state.redb").exists());
}

struct PushedChanges {
    ticks: broadcast::Receiver<TickOutput>,
    pending: VecDeque<PlayerChange>,
    player_inventory: Option<(Vec<Option<ItemStack>>, Option<ItemStack>)>,
}

impl PushedChanges {
    fn new(core: &Core) -> Self {
        Self {
            ticks: core.subscribe(),
            pending: VecDeque::new(),
            player_inventory: None,
        }
    }

    async fn matching(
        &mut self,
        core: &Core,
        description: &str,
        predicate: impl Fn(&PlayerChange) -> bool,
    ) -> PlayerChange {
        tokio::time::timeout(DEADLINE, async {
            loop {
                if let Some(index) = self.pending.iter().position(&predicate) {
                    return self.pending.remove(index).expect("matched queued change");
                }
                // Outboxes are deposited before this tick's broadcast. Never sleep
                // or repeatedly read state: the producer's next publication wakes us.
                self.ticks
                    .recv()
                    .await
                    .expect("tick publisher stays alive without lag");
                for (connection, changes) in core.try_drain_outbox() {
                    assert_eq!(connection, CONNECTION, "only the test player is connected");
                    for change in changes {
                        if let PlayerChange::Chat { message } = &change {
                            eprintln!("plugin-v2 acceptance chat: {message}");
                        }
                        if let PlayerChange::Disconnect { reason, .. } = &change {
                            panic!("acceptance player disconnected: {reason}");
                        }
                        self.pending.push_back(change);
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("missing pushed event: {description}"))
    }

    async fn joined(&mut self, core: &Core) {
        assert!(core.submit(PlayerInput::Join {
            permission_level: 0,
            connection: CONNECTION,
            uuid: PLAYER,
            name: "PluginV2Accept".into(),
        }));
        self.matching(core, "native Joined", |change| {
            matches!(change, PlayerChange::Joined { .. })
        })
        .await;
    }

    async fn chat(&mut self, core: &Core, text: &str) {
        self.matching(
            core,
            text,
            |change| matches!(change, PlayerChange::Chat { message } if message == text),
        )
        .await;
    }

    async fn inventory(&mut self, core: &Core, emeralds: i32, apples: i32) {
        let emerald = core
            .registry()
            .items
            .id_of(&Identifier::parse("minecraft:emerald").unwrap())
            .unwrap();
        let apple = core
            .registry()
            .items
            .id_of(&Identifier::parse("minecraft:apple").unwrap())
            .unwrap();
        loop {
            let change = self
                .matching(core, "authoritative physical player inventory", |change| {
                    matches!(
                        change,
                        PlayerChange::MenuContents { window: 0, .. }
                            | PlayerChange::MenuSlot { window: 0, .. }
                            | PlayerChange::Cursor { .. }
                    )
                })
                .await;
            match change {
                PlayerChange::MenuContents { slots, cursor, .. } => {
                    self.player_inventory = Some((slots, cursor));
                }
                PlayerChange::MenuSlot { slot, item, .. } => {
                    let (slots, _) = self
                        .player_inventory
                        .as_mut()
                        .expect("the native full inventory image precedes its slot deltas");
                    let index =
                        usize::try_from(slot).expect("native inventory slot is nonnegative");
                    *slots
                        .get_mut(index)
                        .expect("native slot lies in its published image") = item;
                }
                PlayerChange::Cursor { item, .. } => {
                    let (_, cursor) = self
                        .player_inventory
                        .as_mut()
                        .expect("the native full inventory image precedes its cursor deltas");
                    *cursor = item;
                }
                _ => unreachable!(),
            }
            let (slots, cursor) = self
                .player_inventory
                .as_ref()
                .expect("native inventory update supplies a full image");
            if count(slots.iter().flatten(), emerald) == emeralds
                && count(slots.iter().flatten(), apple) == apples
                && cursor.is_none()
            {
                return;
            }
        }
    }

    async fn open_shop(&mut self, core: &Core, owned: u32) -> (u8, i32) {
        // Explicit guest feedback also proves unprivileged command dispatch.
        command(core, "economy invalid-acceptance-argument", 0);
        self.chat(core, "Usage: /economy [shop]").await;
        command(core, "economy shop", 0);
        self.shop(core, owned).await
    }

    async fn shop(&mut self, core: &Core, owned: u32) -> (u8, i32) {
        let expected = format!("Apples | buy 3 Emeralds | refund | owned {owned}");
        let change = self
            .matching(
                core,
                "real guest economy menu and durable ledger label",
                |change| {
                    matches!(change, PlayerChange::MenuContents { window, slots, .. }
                if *window != 0 && slots.first().and_then(Option::as_ref)
                    .and_then(|item| item.custom_name.as_ref())
                    .is_some_and(|label| label.contains(&expected)))
                },
            )
            .await;
        let PlayerChange::MenuContents {
            window, state_id, ..
        } = change
        else {
            unreachable!()
        };
        (window, state_id)
    }
}

fn count<'a>(items: impl Iterator<Item = &'a ItemStack>, item: u32) -> i32 {
    items
        .filter(|stack| stack.item_id == item && !stack.is_empty())
        .map(|stack| stack.count)
        .sum()
}

fn command(core: &Core, text: &str, permission_level: u8) {
    assert!(core.submit(PlayerInput::Command {
        connection: CONNECTION,
        command: text.into(),
        permission_level,
    }));
}

fn click(core: &Core, menu: (u8, i32), button: i8) {
    assert!(core.submit(PlayerInput::ContainerClick {
        connection: CONNECTION,
        window: menu.0,
        state_id: menu.1,
        slot: 0,
        button,
        mode: ClickMode::Pickup,
    }));
}

fn native_image(world: &Path) -> mc_world::native::NativeCheckpointImage {
    let mut identity = [0; 32];
    identity[..16].copy_from_slice(b"SolarisV2Native\0");
    identity[16..24].copy_from_slice(&81_i64.to_be_bytes());
    identity[24..].copy_from_slice(&(mc_worldgen::WORLDGEN_REVISION as u64).to_be_bytes());
    // Authoritative open recovers redb after SIGKILL; the offline export reader
    // deliberately refuses that dirty file. This reads only the checkpoint,
    // leaving all pending tick WAL replay to the subsequent real Core::open.
    mc_world::native::NativeStore::open_with_cache(
        &world.join("native/state.redb"),
        identity,
        16 * 1024 * 1024,
    )
    .expect("open actual native checkpoint")
    .read_checkpoint()
    .expect("read native checkpoint image")
}

fn ledger_key() -> String {
    format!("shop:economy:{PLAYER}")
}

fn checkpoint_ledger(image: &mc_world::native::NativeCheckpointImage) -> Option<(String, u64)> {
    let record = image.globals.get("plugins")?;
    let value: serde_json::Value =
        serde_json::from_slice(&record.payload).expect("native plugin image JSON");
    let record = value
        .get("storage")?
        .get("basic-economy")?
        .get(ledger_key())?;
    Some((
        record["value"]
            .as_str()
            .expect("economy ledger value")
            .into(),
        record["revision"]
            .as_u64()
            .expect("economy ledger revision"),
    ))
}

fn assert_wal_purchase(world: &Path, checkpoint_tick: u64) -> u64 {
    let batches =
        mc_core::journal::read_complete_batches_after(&world.join("journal"), checkpoint_tick)
            .expect("read complete durable tick WAL after pre-purchase checkpoint");
    let key = ledger_key();
    let mut revision = None;
    for (_, _, entries) in batches {
        let image = entries.iter().find_map(|entry| match entry {
            mc_core::journal::JournalEntry::Plugins { value } => {
                value.storage_value_version("basic-economy", &key)
            }
            _ => None,
        });
        if let Some((Some(value), version)) = image {
            assert_eq!(value, PURCHASE);
            // The physical player post-image and KV post-image share one atomic
            // tick batch, rather than independent sidefiles or separate WALs.
            let inventory = entries
                .iter()
                .find_map(|entry| match entry {
                    mc_core::journal::JournalEntry::Player {
                        uuid, inventory, ..
                    } if uuid == &PLAYER.to_string() => Some(inventory),
                    _ => None,
                })
                .expect("inventory effect shares the KV WAL batch");
            let items = mc_data::items::solaris_required_items();
            let emerald = items
                .id_of(&Identifier::parse("minecraft:emerald").unwrap())
                .unwrap();
            let apple = items
                .id_of(&Identifier::parse("minecraft:apple").unwrap())
                .unwrap();
            assert_eq!(count(inventory.iter(), emerald), 6);
            assert_eq!(count(inventory.iter(), apple), 2);
            revision = Some(version);
        }
    }
    revision.expect("real guest transaction produced a durable plugin KV post-image")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires deployed first-party basic-economy WASM; explicit sibling integration gate"]
async fn real_sibling_economy_command_wal_and_checkpoint_are_exactly_once() {
    let source = real_economy_package();
    let directory = TestDirectory::new();
    let deployment = directory.0.join("plugins");
    deploy_economy(&source, &deployment);

    // Corrupt the real component, not a successful substitute fixture. Loading
    // must fail even though its genuine manifest and configuration are valid.
    let wasm = deployment.join("basic-economy/plugin.wasm");
    let bytes = std::fs::read(&wasm).unwrap();
    assert!(
        bytes.len() > 8,
        "missing prerequisite: deployed component is empty"
    );
    std::fs::write(&wasm, &bytes[..7]).unwrap();
    expect_plugin_startup_failure(configuration(
        &directory.0.join("invalid-world"),
        &deployment,
    ));
    std::fs::write(&wasm, &bytes).unwrap();

    let world = directory.0.join("world");
    let mut child = CrashWriter(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "plugin_tests::plugin_v2_crash_writer_child",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD_WORLD, &world)
            .env(CHILD_DEPLOY, &deployment)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("launch real Core in crash-isolated test process"),
    );
    let stdout = child.0.stdout.take().unwrap();
    tokio::time::timeout(
        DEADLINE,
        tokio::task::spawn_blocking(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                if line.expect("read crash writer receipt").contains(READY) {
                    return;
                }
            }
            panic!("crash writer exited before durable guest purchase receipt");
        }),
    )
    .await
    .expect("durable guest receipt deadline")
    .expect("crash writer receipt reader");
    child.0.kill().expect("crash without shutdown checkpoint");
    let status = child.0.wait().expect("reap crash writer");
    assert!(
        !status.success(),
        "writer must be killed, never gracefully checkpointed"
    );
    drop(child);

    let baseline = native_image(&world);
    assert_eq!(
        checkpoint_ledger(&baseline),
        None,
        "purchase is strictly WAL-only at crash"
    );
    let revision = assert_wal_purchase(&world, baseline.tick);
    assert_eq!(revision, 1, "one purchase advances the ledger once");

    // First open replays WAL. A second open after a checkpoint must retain the
    // exact revision and physical inventory, with no repeated debit or grant.
    for _ in 0..2 {
        let core = RunningCore(
            Core::open(configuration(&world, &deployment)).expect("recover real plugin world"),
        );
        assert!(
            core.0
                .plugin_commands()
                .iter()
                .any(|(name, _)| name == "economy")
        );
        let mut changes = PushedChanges::new(&core.0);
        changes.joined(&core.0).await;
        changes.inventory(&core.0, 6, 2).await;
        changes.open_shop(&core.0, 1).await;
        assert!(
            core.0.checkpoint(),
            "checkpoint restored physical effect and KV together"
        );
        drop(core);
        assert_eq!(
            checkpoint_ledger(&native_image(&world)),
            Some((PURCHASE.into(), revision))
        );
    }

    let core = RunningCore(Core::open(configuration(&world, &deployment)).unwrap());
    let mut changes = PushedChanges::new(&core.0);
    changes.joined(&core.0).await;
    changes.inventory(&core.0, 6, 2).await;
    let menu = changes.open_shop(&core.0, 1).await;
    click(&core.0, menu, 1);
    changes.chat(&core.0, "Refunded Apples.").await;
    changes.inventory(&core.0, 9, 0).await;
    changes.shop(&core.0, 0).await;
    assert!(core.0.checkpoint());
    drop(core);
    assert_eq!(
        checkpoint_ledger(&native_image(&world)),
        Some(("v2|".into(), revision + 1))
    );

    let core = RunningCore(Core::open(configuration(&world, &deployment)).unwrap());
    let mut changes = PushedChanges::new(&core.0);
    changes.joined(&core.0).await;
    changes.inventory(&core.0, 9, 0).await;
    let menu = changes.open_shop(&core.0, 0).await;
    click(&core.0, menu, 1);
    changes
        .chat(&core.0, "Nothing from this shop is eligible for refund.")
        .await;
    drop(core);
    assert_eq!(
        checkpoint_ledger(&native_image(&world)),
        Some(("v2|".into(), revision + 1)),
        "a repeated refund cannot create currency or advance the KV revision"
    );
    let core = RunningCore(Core::open(configuration(&world, &deployment)).unwrap());
    let mut changes = PushedChanges::new(&core.0);
    changes.joined(&core.0).await;
    changes.inventory(&core.0, 9, 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "internal subprocess helper; invoked only by the real sibling acceptance test"]
async fn plugin_v2_crash_writer_child() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("mc_plugin_host=debug,plugin.command_result=info")
        .with_writer(std::io::stderr)
        .try_init();
    let world = PathBuf::from(std::env::var_os(CHILD_WORLD)
        .expect("missing prerequisite: invoke through real_sibling_economy_command_wal_and_checkpoint_are_exactly_once"));
    let deployment =
        PathBuf::from(std::env::var_os(CHILD_DEPLOY).expect("parent supplies genuine deployment"));
    eprintln!("plugin-v2 acceptance: loading the genuine guest and native world");
    let core = RunningCore(
        Core::open(configuration(&world, &deployment)).expect("load genuine first-party package"),
    );
    assert!(
        core.0
            .plugin_commands()
            .iter()
            .any(|(name, _)| name == "economy")
    );
    eprintln!("plugin-v2 acceptance: guest loaded; joining the native player");
    let mut changes = PushedChanges::new(&core.0);
    changes.joined(&core.0).await;
    eprintln!("plugin-v2 acceptance: player joined; supplying native currency");
    command(&core.0, "give @s minecraft:emerald 9", 4);
    changes.inventory(&core.0, 9, 0).await;
    assert!(
        core.0.checkpoint(),
        "native baseline precedes the guest transaction"
    );
    eprintln!("plugin-v2 acceptance: baseline checkpoint committed; opening the guest shop");
    let menu = changes.open_shop(&core.0, 0).await;
    eprintln!("plugin-v2 acceptance: guest shop opened; purchasing through its native menu");
    click(&core.0, menu, 0);
    changes.chat(&core.0, "Purchased Apples.").await;
    changes.inventory(&core.0, 6, 2).await;
    changes.shop(&core.0, 1).await;
    eprintln!("plugin-v2 acceptance: purchase reply, physical inventory and guest ledger received");
    println!("{READY}");
    std::io::stdout()
        .flush()
        .expect("push durable crash receipt to parent");
    // The parent kills this process. Do not call shutdown/Drop here: that would
    // turn the WAL replay gate into a graceful-checkpoint-only restart test.
    loop {
        std::thread::park();
    }
}

fn copy_deployment_assets(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).expect("create isolated package asset directory");
    for entry in std::fs::read_dir(source).expect("read actual deployed package assets") {
        let entry = entry.expect("read deployed asset entry");
        let kind = entry.file_type().expect("inspect deployed asset");
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_deployment_assets(&entry.path(), &target);
        } else {
            assert!(
                kind.is_file(),
                "missing prerequisite: deployed assets must be real files, not symlinks or special files: {}",
                entry.path().display()
            );
            std::fs::copy(entry.path(), target).expect("copy actual deployed asset bytes");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the genuine first-party chisel package and its Loader bundle"]
async fn real_chisel_package_admits_item_recipe_and_block_tools() {
    let source = real_economy_package()
        .parent()
        .expect("deployed package has a sibling root")
        .join("chisel");
    let directory = TestDirectory::new();
    let target = directory.0.join("plugins").join("chisel");
    for file in ["plugin.toml", "plugin.wasm"] {
        assert!(
            source.join(file).is_file(),
            "missing integration prerequisite: {}; build the genuine chisel component",
            source.join(file).display()
        );
        std::fs::create_dir_all(&target).unwrap();
        std::fs::copy(source.join(file), target.join(file)).unwrap();
    }
    copy_deployment_assets(&source.join("client"), &target.join("client"));
    let core = RunningCore(
        Core::open(configuration(
            &directory.0.join("world"),
            &directory.0.join("plugins"),
        ))
        .expect("the real chisel component must configure and admit its native catalogs"),
    );
    let registry = core.0.registry();
    let items: Vec<_> = registry
        .plugin_items()
        .map(|item| (item.id.to_string(), item.carrier.to_string()))
        .collect();
    assert!(
        items.contains(&("chisel:chisel".to_owned(), "minecraft:stick".to_owned())),
        "the chisel item is admitted over its stick carrier: {items:?}"
    );
    assert!(
        registry
            .recipes
            .iter()
            .any(|recipe| recipe.id.as_str() == "chisel:chisel"),
        "the shaped chisel recipe is in the crafting catalog"
    );
    assert_eq!(
        registry.block_tool_rule_count(),
        232,
        "58 slab+stairs families, four rules each"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires all six genuine first-party packages and their unmodified assets"]
async fn real_sibling_complete_deployment_admits_physical_items_and_work_recipes() {
    // An explicit acceptance roster is not a production bundled-package list.
    // The settlements package is cut over: its six items / 56 recipes now
    // come from the native catalog; the deployment carries six packages.
    const PACKAGES: [&str; 6] = [
        "basic-economy",
        "solaris-audit",
        "solaris-economy",
        "solaris-essentials",
        "solaris-permissions",
        "solaris-towns",
    ];
    let economy = real_economy_package();
    let sibling = economy
        .parent()
        .expect("deployed package has a sibling root");
    let directory = TestDirectory::new();
    let deployment = directory.0.join("plugins");
    for name in PACKAGES {
        let source = sibling.join(name);
        let target = deployment.join(name);
        std::fs::create_dir_all(&target).unwrap();
        for file in ["plugin.toml", "plugin.wasm"] {
            assert!(
                source.join(file).is_file(),
                "missing integration prerequisite: {}; deploy all six genuine first-party packages; no mock guest or configure answer is allowed",
                source.join(file).display()
            );
            std::fs::copy(source.join(file), target.join(file)).unwrap();
        }
        let manifest: toml::Value = std::fs::read_to_string(source.join("plugin.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            manifest["id"].as_str(),
            Some(name),
            "actual deployed package identity"
        );
        assert_eq!(
            manifest["api"].as_str(),
            Some("0.7.0"),
            "actual deployed component API"
        );
        if source.join("config.toml").is_file() {
            std::fs::copy(source.join("config.toml"), target.join("config.toml")).unwrap();
        }
        for assets in ["client", "structures"] {
            let assets_source = source.join(assets);
            if assets_source.is_dir() {
                copy_deployment_assets(&assets_source, &target.join(assets));
            }
        }
    }

    // Discover/configure the genuine six-package deployment; the native
    // settlement catalog installs unconditionally through the same pipeline.
    let core = RunningCore(
        Core::open(configuration(&directory.0.join("world"), &deployment)).expect(
            "all real first-party components must configure and admit their native catalogs",
        ),
    );

    let packages: std::collections::BTreeSet<_> = core
        .0
        .plugin_packages()
        .iter()
        .map(|package| package.plugin_id())
        .collect();
    assert_eq!(packages, PACKAGES.into_iter().collect());
    // Native `/settlement` is core-owned now, so guest roots exclude it; it
    // is asserted separately below alongside the native catalog.
    let commands: std::collections::BTreeSet<_> = core
        .0
        .plugin_commands()
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(
        commands,
        [
            "economy",
            "audit",
            "money",
            "pay",
            "econadmin",
            "sethome",
            "home",
            "delhome",
            "setspawn",
            "spawn",
            "setwarp",
            "warp",
            "delwarp",
            "back",
            "tpa",
            "tpaccept",
            "msg",
            "reply",
            "perm",
            "town",
        ]
        .into_iter()
        .collect(),
        "complete genuine manifest-owned command roots",
    );

    // The six items are native now: same authored identities, but the
    // authority under test is the native startup catalog, not a guest.
    const PHYSICAL_ITEMS: [&str; 6] = [
        "arming_sword",
        "longsword",
        "spear",
        "war_bow",
        "mail_hauberk",
        "pavise",
    ];
    let registry = core.0.registry();
    let items: std::collections::BTreeSet<_> = registry
        .plugin_items()
        .map(|item| item.id.to_string())
        .collect();
    assert_eq!(
        items,
        PHYSICAL_ITEMS
            .into_iter()
            .map(|name| format!("solaris-settlements:{name}"))
            .collect(),
        "all six native settlement physical definitions are admitted",
    );
    for item in registry.plugin_items() {
        assert!(
            item.facts.physical_profile.is_some(),
            "physical profile discarded for {}",
            item.id
        );
        assert!(
            std::ptr::eq(item, registry.item_facts.custom(&item.id).unwrap()),
            "native definitions live in the canonical native item-facts authority",
        );
    }
    // Native `/settlement` dispatches through core command execution, not the
    // guest roots above; dispatch coverage lives in `command_execution_tests`.
    // Here the admitted native catalog proves the native side of the cutover.
    let mut work_recipes: Vec<String> = [
        "oak_planks",
        "stick",
        "bread",
        "sugar",
        "cake",
        "saddle",
        "hay_block",
        "open_hay",
    ]
    .into_iter()
    .map(|name| format!("solaris-settlements:{name}"))
    .collect();
    for name in [
        "wooden_pickaxe",
        "wooden_hoe",
        "wooden_axe",
        "stone_pickaxe",
        "repair_wooden_pickaxe",
        "salvage_wooden_pickaxe",
        "arming_sword",
        "longsword",
        "spear",
        "war_bow",
        "mail_hauberk",
        "pavise",
    ] {
        for quality in ["rough", "ordinary", "good", "masterwork"] {
            work_recipes.push(format!("solaris-settlements:{name}_{quality}"));
        }
    }
    assert_eq!(work_recipes.len(), 56);
    for name in work_recipes {
        let id = Identifier::parse(&name).unwrap();
        let recipe = registry.item_facts.work_recipe(&id).unwrap_or_else(|| {
            panic!("native settlement work recipe missing from native authority: {name}")
        });
        assert_eq!(recipe.id, id);
        assert!(
            recipe.duration_ticks > 0,
            "real native work duration: {name}"
        );
        assert!(
            !recipe.inputs.is_empty(),
            "native custody inputs retained: {name}"
        );
        assert!(
            !recipe.outputs.is_empty(),
            "physical outputs retained: {name}"
        );
    }
    let mut ticks = core.0.subscribe();
    tokio::time::timeout(DEADLINE, ticks.recv())
        .await
        .expect("complete deployment pushed tick deadline")
        .expect("complete deployment tick owner stays alive");
    drop(core);
}
