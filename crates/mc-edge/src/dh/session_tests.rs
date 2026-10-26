use super::*;

#[test]
fn fragment_commit_order_preserves_high_bit_correlation() {
    let budget = Arc::new(Semaphore::new(PLAYER_BYTES));
    let permit = Arc::clone(&budget)
        .try_acquire_many_owned((FRAGMENT + 7) as u32)
        .unwrap();
    let mut transfer = Transfer {
        request: Some(i32::MIN),
        buffer: -7,
        tile: Arc::new(Tile {
            bytes: Arc::new(vec![42; FRAGMENT + 7]),
            modified: 123,
        }),
        offset: 0,
        valid: Arc::new(AtomicBool::new(true)),
        reservation: Arc::new(permit),
    };
    let (message, end) = transfer.message("test@minecraft:overworld".into(), FRAGMENT);
    assert!(
        matches!(message, Message::Split { first: true, buffer: -7, ref data } if data.len() == FRAGMENT)
    );
    transfer.offset = end;
    let (message, end) = transfer.message("test@minecraft:overworld".into(), FRAGMENT);
    assert!(matches!(message, Message::Split { first: false, ref data, .. } if data.len() == 7));
    transfer.offset = end;
    let (message, _) = transfer.message("test@minecraft:overworld".into(), FRAGMENT);
    assert!(matches!(
        message,
        Message::Response {
            id: i32::MIN,
            buffer: Some(-7)
        }
    ));
    transfer.request = None;
    assert!(
        matches!(transfer.message("other-level".into(), FRAGMENT).0, Message::Update { level, .. } if level == "other-level")
    );
}

#[test]
fn canceled_dimension_or_connection_cannot_commit_queued_bytes() {
    let ticket = Arc::new(AtomicBool::new(true));
    let epoch = Arc::new(AtomicBool::new(true));
    let alive = Arc::new(AtomicBool::new(true));
    let outbound = Outbound {
        bytes: bytes::Bytes::from_static(b"queued DTO fragment"),
        valid: Arc::clone(&ticket),
        epoch: Arc::clone(&epoch),
        alive: Arc::clone(&alive),
        _reservation: None,
    };
    assert!(outbound.valid());
    ticket.store(false, Ordering::Release);
    assert!(!outbound.valid());
    ticket.store(true, Ordering::Release);
    epoch.store(false, Ordering::Release);
    assert!(!outbound.valid());
    epoch.store(true, Ordering::Release);
    alive.store(false, Ordering::Release);
    assert!(!outbound.valid());
}

#[test]
fn pending_dto_budget_is_released_only_after_last_queued_fragment() {
    let budget = Arc::new(Semaphore::new(PLAYER_BYTES));
    let permit = Arc::new(
        Arc::clone(&budget)
            .try_acquire_many_owned(PLAYER_BYTES as u32)
            .unwrap(),
    );
    let queued = Arc::clone(&permit);
    assert!(Arc::clone(&budget).try_acquire_owned().is_err());
    drop(permit);
    assert_eq!(budget.available_permits(), 0);
    drop(queued);
    assert_eq!(budget.available_permits(), PLAYER_BYTES);
}

#[test]
fn radius_is_in_chunks_and_world_border_is_authoritative() {
    let border = mc_core::command_border::Border::default();
    let leaf = |x, z| Section { detail: 6, x, z };
    assert!(within_bounds(leaf(32, 0), [0.0, 0.0], 128, border));
    assert!(!within_bounds(leaf(33, 0), [0.0, 0.0], 128, border));
    assert!(within_bounds(leaf(-33, 0), [-1.0, 0.0], 128, border));
    assert!(!within_bounds(leaf(0, 0), [f64::NAN, 0.0], 128, border));
    let small = mc_core::command_border::Border {
        center_x: 0.0,
        center_z: 0.0,
        size: 128.0,
        ..Default::default()
    };
    assert!(within_bounds(leaf(0, 0), [0.0, 0.0], 128, small));
    assert!(!within_bounds(leaf(1, 0), [0.0, 0.0], 128, small));
}

#[test]
fn bandwidth_burst_admits_a_complete_expanding_minecraft_fragment() {
    let mut entropy = 1_u32;
    let data = (0..FRAGMENT)
        .map(|_| {
            entropy = entropy.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (entropy >> 24) as u8
        })
        .collect();
    let frame = super::super::frame(&Message::Split {
        buffer: 1,
        data,
        first: true,
    })
    .unwrap();
    let now = Instant::now();
    let mut bandwidth = Bucket::new(500_000, FRAME_BURST);
    assert!(
        bandwidth.take(frame.len(), now),
        "the {}-byte Minecraft frame must fit the {}-byte burst, even when compression expands a {}-byte DH fragment",
        frame.len(),
        FRAME_BURST,
        FRAGMENT
    );
    assert!(!bandwidth.take(frame.len(), now));
    assert!(bandwidth.take(frame.len(), now + std::time::Duration::from_secs(1)));
}

#[test]
fn warm_client_cache_receives_edits_before_requesting_its_cached_leaf() {
    let mut dirty = HashSet::new();
    let landmark = Section {
        detail: codec::LEAF_DETAIL,
        x: 16,
        z: 0,
    };
    invalidate_area(
        &mut dirty,
        [0.0, 0.0],
        128,
        mc_core::command_border::Border::default(),
        ([15, -1], [17, 1]),
    );
    assert!(
        dirty.contains(&landmark.packed()),
        "warm clients already have the leaf locally and need not request it before a live edit"
    );
}

#[test]
fn invalidation_area_is_bounded_while_a_warm_player_moves() {
    let border = mc_core::command_border::Border::default();
    let mut dirty = HashSet::new();
    invalidate_area(&mut dirty, [0.0, 0.0], 128, border, ([-33, -33], [33, 33]));
    assert_eq!(dirty.len(), 65 * 65);
    let anchor = [1_000_000.0, 0.0];
    invalidate_area(&mut dirty, anchor, 128, border, ([15592, -33], [15658, 33]));
    assert_eq!(dirty.len(), 65 * 65);
    assert!(dirty.iter().all(|position| {
        within_bounds(Section::decode(*position).unwrap(), anchor, 128, border)
    }));
    dirty.clear();
    let small = mc_core::command_border::Border {
        center_x: 0.0,
        center_z: 0.0,
        size: 128.0,
        ..Default::default()
    };
    invalidate_area(&mut dirty, [0.0, 0.0], 128, small, ([-33, -33], [33, 33]));
    assert_eq!(dirty.len(), 4);
}

#[tokio::test]
async fn continuous_edits_and_pending_requests_both_make_progress() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = mc_core::CoreConfig::new(dir.path().join("world"), 42);
    config.vanilla_data_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
    let hub = Arc::new(mc_core::Core::open(config).unwrap());
    let mut cfg = crate::config::EdgeConfig {
        server_dir: dir.path().join("server"),
        ..Default::default()
    };
    cfg.distant_horizons.enabled = true;
    let server = Arc::new(
        EdgeServer::new(
            Arc::clone(&hub),
            Arc::new(mc_data::solaris_required_data()),
            Arc::default(),
            cfg,
        )
        .unwrap(),
    );
    let mut session = Session::new(
        server,
        ConnectionId(1),
        EntityId(1),
        Dimension::Overworld,
        mpsc::channel(128).0,
        Arc::new(AtomicBool::new(true)),
    );
    session.anchor = Some([0.0, 0.0]);
    session.border = Some(mc_core::command_border::Border::default());
    session.offer = Some(Config {
        plan: codec::PLAN_CHUNKS_ONLY,
        distance: 128,
        requests: 2,
        updates: true,
        update_radius: 128,
        sync: true,
        sync_distance: 128,
        sync_requests: 2,
        center_x: 0,
        center_z: 0,
        radius: 0,
        bandwidth: 500,
    });
    let level = session.level_key();
    session.level = Some(level.clone());
    let position = Section {
        detail: codec::LEAF_DETAIL,
        x: 0,
        z: 0,
    }
    .packed();
    let mut ordinary = 0;
    let mut updates = 0;
    for id in 0..64 {
        // Refill the same production bucket and complete admitted jobs
        // explicitly; no wall-clock delays or real terrain workers.
        session.requests.refund(PLAYER_JOBS);
        session.invalidate(([-1, -1], [1, 1]));
        session.request(id, level.clone(), position, None);
        session.schedule();
        assert!(session.jobs.len() + session.transfers.len() <= PLAYER_JOBS);
        for (id, job) in session.jobs.drain() {
            if i32::try_from(id).is_ok() {
                ordinary += 1;
            } else {
                updates += 1;
            }
            job.valid.store(false, Ordering::Release);
            job.abort.abort();
        }
        session.tickets.clear();
        session.tasks.abort_all();
    }
    assert!(
        ordinary >= 32,
        "continuous edits starved requests: {ordinary}"
    );
    assert!(
        updates >= 32,
        "continuous requests starved live edits: {updates}"
    );
    session.cancel_all();
    hub.shutdown();
}
