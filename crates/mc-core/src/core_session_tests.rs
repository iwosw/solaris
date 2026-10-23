use super::*;

#[tokio::test]
async fn leave_waits_for_capacity_instead_of_shedding_lifecycle() {
    let root = tempfile::tempdir().unwrap();
    let mut config = CoreConfig::new(root.path().join("world"), 42);
    config.view_distance = 1;
    config.simulation_distance = 1;
    config.input_queue_cap = 1;
    config.vanilla_data_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
    let core = Core::open(config).unwrap();
    let connection = ConnectionId(77);
    let mut ticks = core.subscribe();
    assert!(core.submit(PlayerInput::Join {
        connection,
        uuid: uuid::Uuid::from_u128(77),
        name: "Lifecycle".into(),
        permission_level: 0,
    }));
    let entity = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let _ = ticks.recv().await;
            for (id, changes) in core.try_drain_outbox() {
                if id == connection {
                    for change in changes {
                        if let PlayerChange::Joined { entity, .. } = change {
                            return entity;
                        }
                    }
                }
            }
        }
    })
    .await
    .unwrap();
    let permit = core.inner.input_tx.reserve().await.unwrap();
    let mut leaving = std::pin::pin!(core.leave(connection));
    std::future::poll_fn(|cx| {
        assert!(leaving.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(permit);
    leaving.await;
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            if let Ok(output) = ticks.recv().await
                && output.snapshot.entities.get(entity).is_none()
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    core.shutdown();
}
