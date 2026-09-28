use super::tests::{profile, register_test_session};
use super::*;
#[tokio::test]
async fn prepared_chunk_wait_wakes_when_cache_is_published() {
    let registry = SessionRegistry::new();
    let chunk = (0, 0);
    let (tx, _rx) = mpsc::channel(1);
    registry.register(
        &profile("PreparedWait"),
        chunk,
        0,
        HashSet::from([chunk]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let observed = registry.prepared_change_generation();
    let mut changed = Box::pin(registry.wait_for_prepared_change(observed));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(changed.as_mut(), cx).is_pending(),
            "prepared wait must stay pending before a chunk is published"
        );
        std::task::Poll::Ready(())
    })
    .await;

    registry.cache_prepared_chunk(
        chunk,
        Arc::new(PreparedChunkFrame {
            frame: Bytes::from_static(b"prepared-chunk"),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
            packet_data_len: 0,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        }),
    );
    tokio::time::timeout(Duration::from_secs(1), changed)
        .await
        .expect("prepared cache publication must wake its waiters");
}

#[test]
fn prepared_cache_publication_does_not_wait_for_session_state() {
    let registry = Arc::new(SessionRegistry::new());
    let chunk = (0, 0);
    let (tx, _rx) = mpsc::channel(1);
    registry.register(
        &profile("PreparedCachePublication"),
        chunk,
        0,
        HashSet::from([chunk]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let prepared = Arc::new(PreparedChunkFrame {
        frame: Bytes::from_static(b"prepared-cache-publication"),
        light: None,
        herd_spawns: Vec::new(),
        hydrated_campfires: Vec::new(),
        packet_data_len: 0,
        build_timing: ChunkBuildTiming::default(),
        write_timing: ChunkWriteTiming::default(),
    });
    let inner = registry.lock_inner("hold session state during prepared cache publication");
    let (published_tx, published_rx) = std::sync::mpsc::channel();
    let publishing_registry = Arc::clone(&registry);
    let publisher = std::thread::spawn(move || {
        let published = publishing_registry.cache_prepared_chunk_if_current(chunk, 0, prepared);
        let _ = published_tx.send(published);
    });

    let published = published_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("prepared cache publication must not wait for session state");
    assert!(published);
    drop(inner);
    publisher.join().expect("prepared cache publisher succeeds");
}

#[test]
fn prewarmed_cache_publication_does_not_wait_for_session_state() {
    let registry = Arc::new(SessionRegistry::new());
    let chunk = (0, 1);
    let prepared = Arc::new(PreparedChunkFrame {
        frame: Bytes::from_static(b"prewarmed-cache-publication"),
        light: None,
        herd_spawns: Vec::new(),
        hydrated_campfires: Vec::new(),
        packet_data_len: 0,
        build_timing: ChunkBuildTiming::default(),
        write_timing: ChunkWriteTiming::default(),
    });
    let inner = registry.lock_inner("hold session state during prewarmed cache publication");
    let (published_tx, published_rx) = std::sync::mpsc::channel();
    let publishing_registry = Arc::clone(&registry);
    let publisher = std::thread::spawn(move || {
        let published = publishing_registry.cache_prewarmed_chunk(chunk, 0, prepared, 1);
        let _ = published_tx.send(published);
    });

    let published = published_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("prewarmed cache publication must not wait for session state");
    assert!(published);
    drop(inner);
    publisher
        .join()
        .expect("prewarmed cache publisher succeeds");
}

#[test]
fn prepared_chunk_is_released_after_all_current_subscribers_load_it() {
    let registry = SessionRegistry::new();
    let chunk = (0, 0);
    let (alice_tx, _alice_rx) = mpsc::channel(1);
    let (bob_tx, _bob_rx) = mpsc::channel(1);
    let (alice, _) = registry.register(
        &profile("PreparedReleaseAlice"),
        chunk,
        0,
        HashSet::from([chunk]),
        alice_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (bob, _) = registry.register(
        &profile("PreparedReleaseBob"),
        chunk,
        0,
        HashSet::from([chunk]),
        bob_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let prepared = Arc::new(PreparedChunkFrame {
        frame: Bytes::from(vec![7; 4_096]),
        light: Some(ChunkLight::filled(15, 0)),
        herd_spawns: Vec::new(),
        hydrated_campfires: Vec::new(),
        packet_data_len: 4_096,
        build_timing: ChunkBuildTiming::default(),
        write_timing: ChunkWriteTiming::default(),
    });
    registry.cache_prepared_chunk(chunk, Arc::clone(&prepared));

    let _ = registry.mark_loaded(alice, chunk);
    assert!(registry.prepared_chunk(chunk).is_some());

    let _ = registry.mark_loaded(bob, chunk);
    assert!(registry.prepared_chunk(chunk).is_none());
    assert!(
        !registry.cache_prepared_chunk_if_current(chunk, 0, prepared),
        "the sender must not reinsert a frame after every current subscriber loaded it"
    );
}

#[test]
fn prewarmed_chunk_remains_available_for_a_trailing_subscriber() {
    let registry = SessionRegistry::new();
    let old_chunk = (0, 0);
    let prewarmed_chunk = (0, 1);
    let (alice_tx, _alice_rx) = mpsc::channel(1);
    let (bob_tx, _bob_rx) = mpsc::channel(1);
    let (alice, _) = registry.register(
        &profile("PrewarmTrailingAlice"),
        old_chunk,
        0,
        HashSet::from([old_chunk]),
        alice_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (bob, _) = registry.register(
        &profile("PrewarmTrailingBob"),
        old_chunk,
        0,
        HashSet::from([old_chunk]),
        bob_tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let prepared = Arc::new(PreparedChunkFrame {
        frame: Bytes::from(vec![7; 4_096]),
        light: Some(ChunkLight::filled(15, 0)),
        herd_spawns: Vec::new(),
        hydrated_campfires: Vec::new(),
        packet_data_len: 4_096,
        build_timing: ChunkBuildTiming::default(),
        write_timing: ChunkWriteTiming::default(),
    });
    assert!(registry.cache_prewarmed_chunk(prewarmed_chunk, 0, prepared, 64));

    registry.replace_view(alice, prewarmed_chunk, 0, HashSet::from([prewarmed_chunk]));
    let _ = registry.mark_loaded(alice, prewarmed_chunk);
    assert!(
        registry.prepared_chunk(prewarmed_chunk).is_some(),
        "a prewarm-derived frame must survive the leading subscriber"
    );

    registry.replace_view(bob, prewarmed_chunk, 0, HashSet::from([prewarmed_chunk]));
    assert!(matches!(
        registry.prepared_chunk_or_wait_for_earlier_session(prewarmed_chunk, bob),
        SessionPreparedChunkClaimResult::Cached(_, 0)
    ));
    assert_eq!(registry.shed_prepared_chunks(), 1);
    assert!(registry.prepared_chunk(prewarmed_chunk).is_none());
}

#[test]
fn prewarmed_cache_hit_refreshes_lru_recency() {
    let registry = SessionRegistry::new();
    let first = (0, 1);
    let second = (1, 1);
    let third = (2, 1);
    let prepared = Arc::new(PreparedChunkFrame {
        frame: Bytes::from(vec![7; 4_096]),
        light: Some(ChunkLight::filled(15, 0)),
        herd_spawns: Vec::new(),
        hydrated_campfires: Vec::new(),
        packet_data_len: 4_096,
        build_timing: ChunkBuildTiming::default(),
        write_timing: ChunkWriteTiming::default(),
    });

    assert!(registry.cache_prewarmed_chunk(first, 0, Arc::clone(&prepared), 2));
    assert!(registry.cache_prewarmed_chunk(second, 0, Arc::clone(&prepared), 2));
    assert!(matches!(
        registry.prepared_chunk_or_claim(first),
        PreparedChunkClaimResult::Cached
    ));

    {
        let cache = registry.lock_prepared_cache("inspect prewarm LRU after cache hit");
        assert_eq!(
            cache.prewarmed_prepared,
            VecDeque::from([second, first]),
            "a cache hit must make the prepared frame most recent"
        );
    }

    assert!(registry.cache_prewarmed_chunk(third, 0, prepared, 2));
    let cache = registry.lock_prepared_cache("inspect prewarm LRU after eviction");
    assert!(cache.prepared.contains_key(&first));
    assert!(!cache.prepared.contains_key(&second));
    assert!(cache.prepared.contains_key(&third));
}

#[test]
fn prewarm_eviction_preserves_active_session_frontier() {
    let registry = SessionRegistry::new();
    let _session = register_test_session(&registry, "PrewarmFrontier");
    let current_side_edge = (3, 0);
    let stale = (100, 100);
    let another_current_edge = (3, 1);
    let prepared = Arc::new(PreparedChunkFrame {
        frame: Bytes::from(vec![7; 4_096]),
        light: Some(ChunkLight::filled(15, 0)),
        herd_spawns: Vec::new(),
        hydrated_campfires: Vec::new(),
        packet_data_len: 4_096,
        build_timing: ChunkBuildTiming::default(),
        write_timing: ChunkWriteTiming::default(),
    });

    assert!(registry.cache_prewarmed_chunk(current_side_edge, 0, Arc::clone(&prepared), 2));
    assert!(registry.cache_prewarmed_chunk(stale, 0, Arc::clone(&prepared), 2));
    assert!(registry.cache_prewarmed_chunk(another_current_edge, 0, prepared, 2));

    let cache = registry.lock_prepared_cache("inspect active prewarm frontier eviction");
    assert!(cache.prepared.contains_key(&current_side_edge));
    assert!(!cache.prepared.contains_key(&stale));
    assert!(cache.prepared.contains_key(&another_current_edge));
}

#[test]
fn unregister_releases_session_owned_prepared_claim() {
    let registry = SessionRegistry::new();
    let chunk = (2, -3);
    let (tx, _rx) = mpsc::channel(1);
    let (session, _) = registry.register(
        &profile("PreparedClaimOwner"),
        chunk,
        0,
        HashSet::from([chunk]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let claim = match registry.prepared_chunk_or_wait_for_earlier_session(chunk, session) {
        SessionPreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected session-owned claim, got {other:?}"),
    };
    assert_eq!(claim.owner_session, Some(session));

    registry.unregister(session);

    let replacement = match registry.prepared_chunk_or_claim(chunk) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected unregister to release session claim, got {other:?}"),
    };
    assert!(registry.release_prepared_chunk_claim(chunk, replacement));
}

#[test]
fn prepared_chunk_claim_blocks_duplicate_until_released() {
    let registry = SessionRegistry::new();
    let chunk = (2, -3);

    let first_claim = match registry.prepared_chunk_or_claim(chunk) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected first claim, got {other:?}"),
    };

    assert!(matches!(
        registry.prepared_chunk_or_claim(chunk),
        PreparedChunkClaimResult::InFlight
    ));

    assert!(!registry.release_prepared_chunk_claim(
        chunk,
        PreparedChunkClaim {
            id: first_claim.id.wrapping_add(1),
            revision: first_claim.revision,
            owner_session: first_claim.owner_session,
        }
    ));
    assert!(matches!(
        registry.prepared_chunk_or_claim(chunk),
        PreparedChunkClaimResult::InFlight
    ));

    assert!(registry.release_prepared_chunk_claim(chunk, first_claim));
    assert!(matches!(
        registry.prepared_chunk_or_claim(chunk),
        PreparedChunkClaimResult::Claimed(_)
    ));
}

#[test]
fn prepared_chunk_invalidation_revokes_inflight_publication() {
    let registry = SessionRegistry::new();
    let chunk = (2, -3);
    let (tx, _rx) = mpsc::channel(1);
    let (session_id, _) = registry.register(
        &profile("PreparedInvalidation"),
        chunk,
        0,
        HashSet::from([chunk]),
        tx,
        PlayerPose::new(32.5, 64.0, -47.5),
    );
    let claim = match registry.prepared_chunk_or_claim(chunk) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected first claim, got {other:?}"),
    };
    let observed = registry.prepared_change_generation();

    registry.invalidate_prepared_chunks(&HashSet::from([chunk]));

    assert!(registry.prepared_change_generation() > observed);
    assert!(!registry.cache_prepared_chunk_if_current(
        chunk,
        claim.revision,
        Arc::new(PreparedChunkFrame {
            frame: Bytes::from_static(b"stale-prepared-chunk"),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
            packet_data_len: 0,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        }),
    ));
    assert!(registry.prepared_chunk(chunk).is_none());
    assert!(
        registry
            .mark_loaded_if_prepared_revision_current(session_id, chunk, claim.revision)
            .is_none()
    );
    assert!(matches!(
        registry.prepared_chunk_or_claim(chunk),
        PreparedChunkClaimResult::InFlight
    ));
    assert!(registry.release_prepared_chunk_claim(chunk, claim));
    assert!(matches!(
        registry.prepared_chunk_or_claim(chunk),
        PreparedChunkClaimResult::Claimed(_)
    ));
}

#[test]
fn invalidating_unique_prewarmed_chunks_does_not_retain_revisions() {
    let registry = SessionRegistry::new();
    let chunks = (0..64).map(|index| (index, -index)).collect::<HashSet<_>>();
    let prepared = Arc::new(PreparedChunkFrame {
        frame: Bytes::from_static(b"unique-prewarmed-chunk"),
        light: None,
        herd_spawns: Vec::new(),
        hydrated_campfires: Vec::new(),
        packet_data_len: 0,
        build_timing: ChunkBuildTiming::default(),
        write_timing: ChunkWriteTiming::default(),
    });
    for &chunk in &chunks {
        assert!(registry.cache_prewarmed_chunk(chunk, 0, Arc::clone(&prepared), chunks.len(),));
    }

    registry.invalidate_prepared_chunks(&chunks);

    let cache = registry.lock_prepared_cache("inspect invalidated prewarm revisions");
    assert!(cache.prewarmed_prepared.is_empty());
    assert!(cache.prepared.is_empty());
    assert_eq!(
        cache.prepared_revisions.len(),
        0,
        "unticketed, non-in-flight invalidated prewarm revisions must remain bounded"
    );
}

#[test]
fn memory_pressure_shed_releases_shared_prepared_frames() {
    let registry = SessionRegistry::new();
    let chunks = [(2, -3), (3, -3)];
    let (tx, _rx) = mpsc::channel(1);
    registry.register(
        &profile("PreparedMemoryShed"),
        chunks[0],
        1,
        HashSet::from(chunks),
        tx,
        PlayerPose::new(32.5, 64.0, -47.5),
    );
    let frames = chunks.map(|chunk| {
        let frame = Arc::new(PreparedChunkFrame {
            frame: Bytes::from(vec![u8::try_from(chunk.0).unwrap(); 4_096]),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
            packet_data_len: 4_096,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        });
        registry.cache_prepared_chunk(chunk, Arc::clone(&frame));
        frame
    });
    assert_eq!(registry.pressure_snapshot().prepared_chunks, 2);
    assert!(frames.iter().all(|frame| Arc::strong_count(frame) == 2));
    let observed = registry.prepared_change_generation();

    assert_eq!(registry.shed_prepared_chunks(), 2);

    assert_eq!(registry.pressure_snapshot().prepared_chunks, 0);
    assert!(frames.iter().all(|frame| Arc::strong_count(frame) == 1));
    assert!(registry.prepared_change_generation() > observed);
}
