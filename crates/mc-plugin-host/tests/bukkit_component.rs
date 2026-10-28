//! Opt-in receipt: a real upstream-API JAR converted through the ordinary WIT host.

use mc_plugin_host::bindings::exports::solaris::plugin::events::{
    CommandInvoked, Event, EventContext, PlayerJoined,
};
use mc_plugin_host::bindings::exports::solaris::plugin::lifecycle::InitContext;
use mc_plugin_host::bindings::solaris::plugin::commands::{Command, MessageTarget};
use mc_plugin_host::bindings::solaris::plugin::types::Position;
use mc_plugin_host::package::compile_package;
use mc_plugin_host::{
    HostServices, LogLevel, PluginInstance, PluginLimits, PluginStartup, engine, linker,
    load_package,
};
use mc_script::precommit::{BuildContext, BuildEdit, HookActor, HookDecision, HookPlayer};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
struct Services {
    lines: Arc<Mutex<Vec<String>>>,
}

impl HostServices for Services {
    fn log(&mut self, _level: LogLevel, message: &str) {
        self.lines
            .lock()
            .expect("log mutex")
            .push(message.to_owned());
    }

    fn plugin_id(&self) -> &str {
        "solarisbukkithello"
    }
}

#[test]
#[ignore = "requires SOLARIS_BUKKIT_PACKAGE and SOLARIS_BUKKIT_BLOCKS from the offline converter"]
fn converted_jar_runs_through_existing_component_host() {
    let path = std::env::var_os("SOLARIS_BUKKIT_PACKAGE").expect("converted fixture package");
    let blocks =
        std::env::var_os("SOLARIS_BUKKIT_BLOCKS").expect("matching deployment blocks report");
    let blocks: serde_json::Value =
        serde_json::from_slice(&std::fs::read(blocks).expect("blocks report"))
            .expect("registry JSON");
    let state = |name: &str| {
        u32::try_from(blocks[name]["states"][0]["id"].as_u64().expect("state id"))
            .expect("u32 state id")
    };
    let limits = PluginLimits::default();
    let engine = engine(&limits).expect("ordinary bounded engine");
    let package = load_package(Path::new(&path), &limits).expect("generated native manifest");
    let compiled = compile_package(&engine, &package, &limits)
        .expect("TeaVM plus SDK adapter must be a real WIT component");
    let linker = linker::<Services>(&engine).expect("ordinary component linker");
    let services = Services::default();
    let startup =
        PluginStartup::instantiate(&linker, compiled.component(), services.clone(), limits)
            .expect("ordinary isolated startup store")
            .configure("")
            .expect("configure");
    assert!(startup.is_none());
    assert!(services.lines.lock().expect("logs").is_empty());
    let mut plugin =
        PluginInstance::instantiate(&linker, compiled.component(), services.clone(), limits)
            .expect("ordinary runtime instance");
    let version = mc_script::COMPONENT_PLUGIN_API_VERSION;
    let batch = plugin
        .init(
            "",
            InitContext {
                plugin_id: "solarisbukkithello".to_owned(),
                api_version: format!(
                    "{}.{}.{}",
                    version.major(),
                    version.minor(),
                    version.patch()
                ),
                world_fingerprint: String::new(),
            },
        )
        .expect("Java onEnable through WIT lifecycle init");
    assert!(batch.into_commands().is_empty());
    assert_eq!(*services.lines.lock().expect("logs"), ["BUKKIT_ON_ENABLE"]);

    let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let events = [
        Event::PlayerJoined(PlayerJoined {
            player: uuid.to_owned(),
            session: 7,
            name: "Ada".to_owned(),
        }),
        Event::CommandInvoked(CommandInvoked {
            player: uuid.to_owned(),
            session: 9,
            username: "Béa".to_owned(),
            operator: false,
            position: Position {
                x: 0.0,
                y: 64.0,
                z: 0.0,
            },
            dimension: "minecraft:overworld".to_owned(),
            name: "hello".to_owned(),
            arguments: vec!["one".to_owned(), "two".to_owned()],
            raw_arguments: "one two".to_owned(),
        }),
    ];
    let commands = plugin
        .on_events(
            EventContext {
                tick: 42,
                first_sequence: 0,
                count: 2,
            },
            &events,
        )
        .expect("Java listener and command executor")
        .into_commands();
    assert_eq!(commands.len(), 2);
    for (command, (session, text)) in commands
        .iter()
        .zip([(7, "Welcome, Ada!"), (9, "Hello, Béa! one two")])
    {
        let Command::SendMessage(message) = command else {
            panic!("expected staged message")
        };
        assert!(matches!(message.target, MessageTarget::Session(value) if value == session));
        assert_eq!(message.text, text);
    }
    assert!(
        plugin
            .on_events(
                EventContext {
                    tick: 43,
                    first_sequence: 0,
                    count: 0
                },
                &[]
            )
            .expect("empty callback")
            .into_commands()
            .is_empty(),
        "effects do not leak across calls"
    );

    let player = HookActor::Player(HookPlayer::try_new(uuid, 7).expect("player"));
    let diamond = state("minecraft:diamond_ore");
    let stone = state("minecraft:stone");
    let air = state("minecraft:air");
    for (actor, edits, expected) in [
        (
            player.clone(),
            vec![BuildEdit::new(1, 64, 2, diamond, air)],
            HookDecision::Cancel,
        ),
        (
            player.clone(),
            vec![BuildEdit::new(1, 64, 2, stone, air)],
            HookDecision::Keep,
        ),
        (
            player.clone(),
            vec![BuildEdit::new(1, 64, 2, air, diamond)],
            HookDecision::Keep,
        ),
        (
            HookActor::Environment,
            vec![BuildEdit::new(1, 64, 2, diamond, air)],
            HookDecision::Keep,
        ),
        (
            player.clone(),
            vec![
                BuildEdit::new(1, 64, 2, stone, air),
                BuildEdit::new(2, 64, 2, diamond, air),
            ],
            HookDecision::Cancel,
        ),
    ] {
        let context =
            BuildContext::try_new(actor, "minecraft:overworld", edits).expect("build snapshot");
        let answer = plugin
            .before_build(&context, Instant::now() + Duration::from_millis(100))
            .expect("real Java precommit handler");
        assert_eq!(answer.decision, expected);
        assert!(answer.expected_plugin_storage_versions.is_empty());
    }
    assert!(plugin.retired_because().is_none());
    plugin.shutdown().expect("Java onDisable");
    assert_eq!(
        *services.lines.lock().expect("logs"),
        ["BUKKIT_ON_ENABLE", "BUKKIT_ON_DISABLE"]
    );

    // Unknown custom block IDs must fault, never turn into an arbitrary material.
    let mut unknown = PluginInstance::instantiate(&linker, compiled.component(), services, limits)
        .expect("separate instance");
    unknown
        .init(
            "",
            InitContext {
                plugin_id: "solarisbukkithello".to_owned(),
                api_version: format!(
                    "{}.{}.{}",
                    version.major(),
                    version.minor(),
                    version.patch()
                ),
                world_fingerprint: String::new(),
            },
        )
        .expect("init");
    let context = BuildContext::try_new(
        player,
        "minecraft:overworld",
        vec![BuildEdit::new(0, 64, 0, u32::MAX, air)],
    )
    .expect("unknown build state");
    assert!(
        unknown
            .before_build(&context, Instant::now() + Duration::from_millis(100))
            .is_err()
    );
    assert!(unknown.retired_because().is_some());
}
