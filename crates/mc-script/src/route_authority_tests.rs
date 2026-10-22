use super::*;

fn manifest() -> ValidatedScriptPluginManifest {
    crate::ScriptPluginManifest::new(
        "owner",
        "Owner",
        "0.1.0",
        crate::COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_player_command_root("owned")
    .validate()
    .unwrap()
}

#[test]
fn route_reads_do_not_wait_for_a_writer_and_retained_snapshots_stay_immutable() {
    let authority = PluginRouteAuthority::new();
    authority.register(&manifest()).unwrap();
    let original = authority.snapshot();
    let generation = original.registrations["owner"].generation;
    {
        let _writer = authority.writer.lock().expect("writer is free");
        assert_eq!(authority.roots(false), ["owned"]);
        assert!(authority.owner("owned").is_some());
        assert!(!authority.has_channel("test:none"));
        assert!(authority.registration_is_live("owner", generation));
    }
    authority.unregister("owner");
    assert!(authority.owner("owned").is_none());
    assert!(!authority.registration_is_live("owner", generation));
    assert!(original.commands.contains_key("owned"));
    authority.register(&manifest()).unwrap();
    assert!(!authority.registration_is_live("owner", generation));
}

#[test]
fn poisoned_writer_disables_observation_admission_and_route_readers() {
    let (boundary, endpoint) =
        script_boundary_pair(NonZeroUsize::new(1).unwrap(), NonZeroUsize::new(1).unwrap());
    endpoint.register_plugin_routes(&manifest()).unwrap();
    endpoint.poison_plugin_routes_for_test();
    boundary.enqueue_observation(ScriptEvent::server_started());
    assert!(
        boundary
            .observations
            .lock()
            .expect("queue is free")
            .plugins
            .is_empty()
    );
    assert!(endpoint.plugin_routes.registration("owner").is_none());
    assert!(endpoint.plugin_routes.roots(false).is_empty());
}

fn observer(id: &str) -> ValidatedScriptPluginManifest {
    crate::ScriptPluginManifest::new(id, id, "0.1.0", crate::COMPONENT_PLUGIN_API_VERSION)
        .subscribe_event("server.started")
        .validate()
        .unwrap()
}

#[test]
fn observations_wait_for_acknowledged_tick_and_fan_out_in_round_robin_order() {
    let (boundary, mut endpoint) =
        script_boundary_pair(NonZeroUsize::new(1).unwrap(), NonZeroUsize::new(1).unwrap());
    endpoint.register_plugin_routes(&observer("alpha")).unwrap();
    endpoint.register_plugin_routes(&observer("beta")).unwrap();
    boundary.enqueue_observation(ScriptEvent::server_started());
    assert!(matches!(
        endpoint.try_recv_input(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    boundary.seal_observations(7);
    assert!(matches!(
        endpoint.try_recv_input(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    boundary.try_enqueue_latest_server_tick(7).unwrap();
    assert_eq!(
        endpoint.recv_event_blocking(),
        Some(ScriptEvent::server_tick(7))
    );
    for id in ["alpha", "beta"] {
        let Some(ScriptHostInput::Event(event)) = endpoint.next_observation() else {
            panic!("acknowledged observation reaches each subscriber");
        };
        assert_eq!(event.event.target_plugin_id(), Some(id));
        assert_eq!(event.event.event_name(), "server.started");
    }
    assert!(endpoint.next_observation().is_none());
}

#[test]
fn observation_overflow_is_per_registration_and_replacement_discards_old_events() {
    let (boundary, mut endpoint) =
        script_boundary_pair(NonZeroUsize::new(1).unwrap(), NonZeroUsize::new(1).unwrap());
    let manifest = observer("owner");
    endpoint.register_plugin_routes(&manifest).unwrap();
    for _ in 0..=MAX_OBSERVATIONS_PER_PLUGIN {
        boundary.enqueue_observation(ScriptEvent::server_started());
    }
    assert_eq!(boundary.observation_events_dropped("owner"), 1);
    endpoint.unregister_plugin_routes("owner");
    endpoint.register_plugin_routes(&manifest).unwrap();
    boundary.enqueue_observation(ScriptEvent::server_started());
    assert_eq!(boundary.observation_events_dropped("owner"), 0);
    boundary.seal_observations(9);
    endpoint.highest_delivered_tick = Some(9);
    assert!(endpoint.next_observation().is_some());
    assert!(endpoint.next_observation().is_none());
}
