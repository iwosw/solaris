use super::*;

#[test]
fn default_simulation_channel_is_bounded() {
    let (handle, _owner) = simulation_channel();
    assert_eq!(handle.snapshot().capacity, 1024);
    assert_eq!(handle.snapshot().depth, 0);
}

#[test]
fn simulation_channel_rejects_zero_capacity() {
    assert!(std::panic::catch_unwind(|| simulation_channel_with_capacity(0)).is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn command_arrival_wakes_owner_and_preserves_the_envelope() {
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let mut wake = Box::pin(owner.wait_for_command());
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(wake.as_mut(), cx).is_pending(),
            "empty command queue must keep the owner parked"
        );
        std::task::Poll::Ready(())
    })
    .await;

    let _response = handle.enqueue(claim_xp(1, 10)).expect("command fits");
    assert!(wake.as_mut().await, "command arrival must wake the owner");
    drop(wake);

    let batch = owner.drain_batch(1);
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].sequence, 0);
    assert_eq!(handle.snapshot().depth, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn unbound_handle_rejects_player_command_before_enqueue() {
    let (handle, _owner) = simulation_channel();

    assert!(matches!(
        handle
            .pickup_item_into_inventory(EntityId(1), 42, None, Vec::new(), 64)
            .await,
        Err(SimulationRequestError::InvalidCommand)
    ));
    assert_eq!(handle.snapshot().enqueued, 0);
    assert_eq!(handle.snapshot().depth, 0);
}

#[test]
fn full_simulation_channel_rejects_without_growing_depth() {
    let (handle, _owner) = simulation_channel_with_capacity(1);
    let _first = handle.enqueue(claim_xp(1, 10)).expect("first command fits");
    let error = handle
        .enqueue(claim_xp(2, 11))
        .expect_err("second command must fail closed");

    assert_eq!(error, SimulationRequestError::Full);
    assert_eq!(handle.snapshot().depth, 1);
    assert_eq!(handle.snapshot().rejected_full, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn waiting_sender_is_closed_when_owner_drops() {
    let (handle, owner) = simulation_channel_with_capacity(1);
    let _first = handle.enqueue(claim_xp(1, 10)).expect("first command fits");
    let session_handle = handle.for_session(7);
    let mut waiting = Box::pin(
        session_handle
            .enqueue_player_command_wait(SimulationCommand::SetWorldTime { world_time: 1 }),
    );

    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(waiting.as_mut(), cx).is_pending(),
            "full queue must hold the sender until capacity or closure"
        );
        std::task::Poll::Ready(())
    })
    .await;

    drop(owner);

    assert!(matches!(
        waiting.await,
        Err(SimulationRequestError::OwnerStopped)
    ));
    assert_eq!(handle.snapshot().depth, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn waiting_detached_sender_is_closed_when_owner_shuts_down() {
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let _first = handle.enqueue(claim_xp(1, 10)).expect("first command fits");
    let mut waiting =
        Box::pin(handle.enqueue_detached_wait(SimulationCommand::SetWorldTime { world_time: 1 }));

    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(waiting.as_mut(), cx).is_pending(),
            "full queue must hold the detached sender until capacity or closure"
        );
        std::task::Poll::Ready(())
    })
    .await;

    owner.shutdown();

    assert!(matches!(
        waiting.await,
        Err(SimulationRequestError::ShuttingDown)
    ));
    assert_eq!(handle.snapshot().depth, 0);
}

#[tokio::test]
async fn owner_drop_rejects_pending_response_and_closes_queue() {
    let (handle, owner) = simulation_channel_with_capacity(1);
    let response = handle.enqueue(claim_xp(1, 10)).expect("command fits");

    drop(owner);

    assert!(matches!(
        response.await.expect("owner drop response"),
        Err(SimulationRequestError::OwnerStopped)
    ));
    assert!(matches!(
        handle.enqueue(claim_xp(2, 11)),
        Err(SimulationRequestError::OwnerStopped)
    ));
    assert_eq!(handle.snapshot().depth, 0);
}

#[test]
fn full_simulation_channel_rejects_attack_without_damage() {
    let registry = SessionRegistry::new();
    let target = seed_attack_target(&registry);
    let (handle, _owner) = simulation_channel_with_capacity(1);
    let _occupant = handle.enqueue(claim_xp(1, 10)).expect("first command fits");

    let error = handle
        .enqueue(SimulationCommand::AttackServerEntity {
            entity_id: target,
            damage: 20.0,
            knockback_origin: Some(Vec3::new(0.5, 64.0, 0.5)),
            rewards: EntityKillRewards::default(),
        })
        .expect_err("attack must fail closed while queue is full");
    let health = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == target)
        .expect("target remains")
        .snapshot
        .health;

    assert_eq!(error, SimulationRequestError::Full);
    assert_eq!(health, 20.0);
    assert_eq!(handle.snapshot().rejected_full, 1);
}

#[test]
fn owner_drains_commands_in_monotonic_sequence_order() {
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let _first = handle.enqueue(claim_xp(1, 10)).unwrap();
    let _second = handle.enqueue(claim_xp(2, 11)).unwrap();

    let batch = owner.drain_batch(2);

    assert_eq!(
        batch
            .iter()
            .map(|command| command.sequence)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(handle.snapshot().depth, 0);
    assert_eq!(handle.snapshot().dequeued, 2);
    assert_eq!(handle.snapshot().max_batch, 2);
}

#[tokio::test(flavor = "current_thread")]
async fn waiting_sender_is_admitted_after_earlier_command_drains() {
    let registry = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let _first = handle.enqueue(claim_xp(1, 10)).expect("first command fits");
    let session_handle = handle.for_session(7);
    let mut waiting = Box::pin(
        session_handle
            .enqueue_player_command_wait(SimulationCommand::SetWorldTime { world_time: 1 }),
    );

    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(waiting.as_mut(), cx).is_pending(),
            "full queue must hold the later sender"
        );
        std::task::Poll::Ready(())
    })
    .await;

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let _response = waiting.await.expect("capacity release admits sender");
    let batch = owner.drain_batch(1);

    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].sequence, 1);
}

#[tokio::test]
async fn zero_budget_leaves_queued_command_unprocessed() {
    let registry = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle.enqueue(claim_xp(1, 10)).expect("command fits");

    let report = owner.process_tick(&registry, 0);

    assert_eq!(report.processed, 0);
    assert_eq!(report.remaining_depth, 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(1), response)
            .await
            .is_err()
    );
    assert_eq!(handle.snapshot().max_batch, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_drains_prefetched_deferred_and_receiver_commands() {
    let (handle, mut owner) = simulation_channel_with_capacity(3);
    handle
        .ensure_chunk_herd((1, 1), Vec::new())
        .expect("background herd command fits");
    assert!(owner.drain_ready_batch(1).is_empty());

    let prefetched = handle
        .enqueue(claim_xp(1, 10))
        .expect("prefetched command fits");
    assert!(owner.wait_for_command().await);
    let queued = handle
        .enqueue(claim_xp(2, 11))
        .expect("receiver command fits");

    owner.shutdown();

    assert!(matches!(
        prefetched.await.expect("prefetched shutdown response"),
        Err(SimulationRequestError::ShuttingDown)
    ));
    assert!(matches!(
        queued.await.expect("receiver shutdown response"),
        Err(SimulationRequestError::ShuttingDown)
    ));
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.depth, 0);
    assert_eq!(snapshot.rejected_shutdown, 3);
    assert_eq!(snapshot.dequeued, 3);
}

#[test]
fn cancelled_request_is_removed_before_owner_application() {
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle.enqueue(claim_xp(1, 10)).unwrap();
    drop(response);

    assert!(owner.drain_batch(1).is_empty());
    assert_eq!(handle.snapshot().cancelled, 1);
    assert_eq!(handle.snapshot().depth, 0);
}
