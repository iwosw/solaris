//! Advance the watchdog while the guest is in a host import: no sleeps or CPU races.
mod fixture;

use mc_plugin_host::bindings::exports::solaris::plugin::{events, lifecycle};
use mc_plugin_host::{
    CompiledPlugin, HostError, HostServices, LogLevel, PluginInstance, PluginLimits,
};

struct PausedHost {
    engine: wasmtime::Engine,
    epochs_per_import: u64,
}

impl HostServices for PausedHost {
    fn log(&mut self, _level: LogLevel, message: &str) {
        if message == "block" {
            for _ in 0..self.epochs_per_import {
                self.engine.increment_epoch();
            }
        }
    }

    fn plugin_id(&self) -> &str {
        "budget-test"
    }
}

fn guest(limits: PluginLimits, epochs_per_import: u64) -> PluginInstance<PausedHost> {
    let engine = mc_plugin_host::engine(&limits).unwrap();
    let compiled =
        CompiledPlugin::compile(&engine, &fixture::component_bytes(), &limits, "0.8.0").unwrap();
    let linker = mc_plugin_host::linker::<PausedHost>(&engine).unwrap();
    PluginInstance::instantiate(
        &linker,
        compiled.component(),
        PausedHost {
            engine,
            epochs_per_import,
        },
        limits,
    )
    .unwrap()
}

fn initialize(
    guest: &mut PluginInstance<PausedHost>,
    config: &str,
) -> Result<mc_plugin_host::CommandBatch, HostError> {
    guest.init(
        config,
        lifecycle::InitContext {
            plugin_id: "budget-test".into(),
            api_version: "0.8.0".into(),
            world_fingerprint: String::new(),
        },
    )
}

fn callback(
    guest: &mut PluginInstance<PausedHost>,
) -> Result<mc_plugin_host::CommandBatch, HostError> {
    guest.on_events(
        events::EventContext {
            tick: 1,
            first_sequence: 0,
            count: 1,
        },
        &[events::Event::PlayerJoined(events::PlayerJoined {
            player: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
            session: 7,
            name: "Ada".into(),
        })],
    )
}

#[test]
fn ordinary_callback_survives_host_scheduling_jitter() {
    // Eight host imports each advance eight 25ms epochs (1.6s total),
    // with exactly the same guest instructions as an uncontended invocation.
    let mut guest = guest(PluginLimits::default(), 8);
    initialize(&mut guest, "mode = \"block\"\n").unwrap();
    callback(&mut guest).expect("host scheduling delay must not retire a bounded guest");
    callback(&mut guest).expect("the next callback still serves players");
    assert!(guest.retired_because().is_none());
}

#[test]
fn legacy_four_epoch_budget_reproduces_retirement_after_host_work() {
    let mut guest = guest(
        PluginLimits {
            epoch_ticks_per_call: 4,
            ..Default::default()
        },
        8,
    );
    initialize(&mut guest, "mode = \"block\"\n").unwrap();
    assert!(matches!(callback(&mut guest), Err(HostError::Budget)));
    assert!(matches!(guest.retired_because(), Some(HostError::Budget)));
    assert!(callback(&mut guest).is_err());
}

#[test]
fn generous_epoch_hang_guard_still_retires_a_callback() {
    let limits = PluginLimits::default();
    let mut guest = guest(limits, limits.epoch_ticks_per_call);
    initialize(&mut guest, "mode = \"block\"\n").unwrap();
    assert!(matches!(callback(&mut guest), Err(HostError::Budget)));
    assert!(matches!(guest.retired_because(), Some(HostError::Budget)));
}

#[test]
fn fuel_stops_infinite_guest_work_without_any_epoch_ticks() {
    let mut guest = guest(PluginLimits::default(), 0);
    assert!(matches!(
        initialize(&mut guest, "mode = \"spin\"\n"),
        Err(HostError::Budget)
    ));
    assert!(matches!(guest.retired_because(), Some(HostError::Budget)));
}
