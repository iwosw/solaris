use super::*;

/// A stalled journal receiver parks producers instead of growing the
/// queue: the bounded [`journal_work_channel`] refuses `try_send` while
/// full, a blocking `send` completes only after a drain frees a slot, and
/// every accepted batch arrives exactly once in FIFO order. No sleeps or
/// elapsed-time assertions: full/empty are channel state, and thread
/// `join` is the admission event.
#[test]
fn stalled_journal_receiver_applies_backpressure_and_drains_fifo() {
    let (work_tx, work_rx) = journal_work_channel();
    for tick in 0..JOURNAL_WORK_QUEUE as u64 {
        work_tx
            .send(JournalWork::Append {
                tick_lo: tick,
                tick_hi: tick,
                entries: Vec::new(),
                plugin_ack: None,
                reserved: 0,
            })
            .unwrap();
    }
    // Bounded: with the receiver stalled and the queue full, no further
    // batch is admitted and nothing accepted is dropped.
    assert!(matches!(
        work_tx.try_send(JournalWork::Shutdown),
        Err(std::sync::mpsc::TrySendError::Full(_))
    ));
    let blocked = std::thread::spawn({
        let work_tx = work_tx.clone();
        move || work_tx.send(JournalWork::Shutdown)
    });
    // Still no drain, so the producer cannot have been admitted yet: the
    // queue is still full and its thread still parked in `send`.
    assert!(matches!(
        work_tx.try_send(JournalWork::Append {
            tick_lo: u64::MAX,
            tick_hi: u64::MAX,
            entries: Vec::new(),
            plugin_ack: None,
            reserved: 0,
        }),
        Err(std::sync::mpsc::TrySendError::Full(_))
    ));
    assert!(!blocked.is_finished());
    // Drain one batch: the parked producer is admitted exactly once, and
    // everything arrives in FIFO order.
    let first = work_rx.recv().unwrap();
    blocked.join().expect("journal producer").unwrap();
    let drained: Vec<JournalWork> = std::iter::once(first)
        .chain((0..JOURNAL_WORK_QUEUE).map(|_| work_rx.recv().unwrap()))
        .collect();
    assert!(
        work_rx.try_recv().is_err(),
        "nothing beyond the admitted batches"
    );
    for (index, work) in drained.into_iter().enumerate() {
        if index < JOURNAL_WORK_QUEUE {
            match work {
                JournalWork::Append {
                    tick_lo, tick_hi, ..
                } => {
                    assert_eq!((tick_lo, tick_hi), (index as u64, index as u64));
                }
                JournalWork::Checkpoint(_)
                | JournalWork::SaveGenerated(_)
                | JournalWork::Backup(_)
                | JournalWork::Shutdown => panic!("journal FIFO order broken"),
            }
        } else {
            assert!(matches!(work, JournalWork::Shutdown));
        }
    }
    // Disconnect still fails sends: the fail-closed journal-death signal
    // the tick loop relies on is preserved.
    drop(work_rx);
    assert!(work_tx.send(JournalWork::Shutdown).is_err());
}

fn block_batch(len: usize) -> Vec<JournalEntry> {
    (0..len)
        .map(|index| JournalEntry::Block {
            pos: [index as i32, 0, 0],
            state: 1,
        })
        .collect()
}

/// The count cap (128) alone would admit these batches; the byte budget parks
/// the producer once two batches are in flight and resumes it exactly when the
/// journal side releases a drained batch. Nothing is dropped, order is FIFO, a
/// batch larger than the whole budget is still admitted alone, and closing the
/// budget (journal thread death) fails a parked producer.
#[test]
fn oversized_batches_block_sender_until_bytes_are_released() {
    use crate::journal_budget::entries_bytes;
    let (work_tx, work_rx) = journal_work_channel();
    let bytes = entries_bytes(&block_batch(1000));
    let budget = Arc::new(JournalBudget::new(bytes * 2));
    for tick in 0..2 {
        send_append(&work_tx, &budget, tick, block_batch(1000), None).unwrap();
    }
    // Plenty of count slots remain, yet no third batch fits the bytes.
    assert_eq!(budget.try_reserve(bytes), Ok(false));
    let blocked = std::thread::spawn({
        let (work_tx, budget) = (work_tx.clone(), Arc::clone(&budget));
        move || send_append(&work_tx, &budget, 2, block_batch(1000), None)
    });
    assert!(!blocked.is_finished());
    // The journal thread drains and releases the first batch: the parked
    // producer is admitted, and all three arrive in FIFO order.
    let JournalWork::Append {
        tick_lo, reserved, ..
    } = work_rx.recv().unwrap()
    else {
        panic!("expected append");
    };
    assert_eq!(tick_lo, 0);
    budget.release(reserved);
    blocked.join().expect("journal producer").unwrap();
    for expected in 1..3 {
        let JournalWork::Append { tick_lo, .. } = work_rx.recv().unwrap() else {
            panic!("expected append");
        };
        assert_eq!(tick_lo, expected);
    }
    assert!(work_rx.try_recv().is_err(), "nothing beyond the admitted");
    // Batches 1 and 2 are still in flight (budget full); a dead journal
    // thread fails a parked producer instead of hanging it.
    let parked = std::thread::spawn({
        let (work_tx, budget) = (work_tx.clone(), Arc::clone(&budget));
        move || send_append(&work_tx, &budget, 3, block_batch(1000), None)
    });
    budget.close();
    assert!(parked.join().expect("journal producer").is_err());
    // A single batch above the whole budget is admitted when nothing is in flight.
    let tiny = JournalBudget::new(bytes / 2);
    assert_eq!(tiny.try_reserve(bytes), Ok(true));
    assert_eq!(tiny.try_reserve(1), Ok(false));
}

/// A few heap-heavy entries (NBT, named/componented stacks, an entity image)
/// reserve at least the heap they own, so they alone exhaust the budget and
/// block the sender until the journal side releases exactly what was
/// reserved, while a count of their slots (the old bound) would admit them.
#[test]
fn heap_heavy_entries_reserve_their_payload_and_block_the_sender() {
    use crate::journal_budget::entries_bytes;
    const NBT: usize = 1 << 20;
    let mut stack = mc_data::ItemStack::new(1, 1);
    stack.custom_name = Some("n".repeat(NBT).into_boxed_str());
    let components = mc_data::item_stack::ExtendedItemComponents {
        custom_data: Some(vec![0; NBT]),
        ..Default::default()
    };
    let mut component_stack = mc_data::ItemStack::new(2, 1);
    component_stack.components = Some(Box::new(components));
    let appearance = crate::journal::EntityAppearanceImage {
        equipment: vec![Some(stack.clone()); 2],
        ..Default::default()
    };
    let entity = JournalEntry::Entity {
        id: 1,
        uuid: String::new(),
        type_id: 0,
        health: 1.0,
        alive: true,
        age: 0,
        breed_cooldown: 0,
        pickup_delay: 0,
        item: None,
        appearance,
        active_effects: Default::default(),
        villager: None,
        state: Default::default(),
        projectile: None,
        tick: 0,
        pos: [0.0; 3],
        vel: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
    };
    let batch = move || {
        vec![
            JournalEntry::OpaqueBlockEntity {
                pos: [0, 0, 0],
                nbt: Some(vec![0; NBT]),
            },
            JournalEntry::BlockEntity {
                pos: [1, 0, 0],
                slots: vec![component_stack.clone()],
                furnace: None,
                tick: 0,
            },
            entity.clone(),
        ]
    };
    // NBT + component custom_data + two 1 MiB names (entity equipment).
    let heap = 4 * NBT;
    let bytes = entries_bytes(&batch());
    assert!(bytes >= heap, "estimate {bytes} undercounts heap {heap}");
    let (work_tx, work_rx) = journal_work_channel();
    let budget = Arc::new(JournalBudget::new(bytes + bytes / 2));
    send_append(&work_tx, &budget, 0, batch(), None).unwrap();
    // Three entry slots only; the payload alone fills the budget.
    assert_eq!(budget.try_reserve(bytes), Ok(false));
    let blocked = std::thread::spawn({
        let (work_tx, budget, batch) = (work_tx.clone(), Arc::clone(&budget), batch.clone());
        move || send_append(&work_tx, &budget, 1, batch(), None)
    });
    assert!(!blocked.is_finished());
    let JournalWork::Append { reserved, .. } = work_rx.recv().unwrap() else {
        panic!("expected append");
    };
    assert_eq!(reserved, bytes);
    budget.release(reserved);
    blocked.join().expect("journal producer").unwrap();
}

/// One `Append` of `entries` reserves at least `heap` bytes, fills a budget
/// sized 1.5x its estimate, and blocks the next sender until the journal side
/// releases exactly the reserved number.
fn assert_reserves_heap_and_blocks(entries: Vec<JournalEntry>, heap: usize) {
    let bytes = crate::journal_budget::entries_bytes(&entries);
    assert!(bytes >= heap, "estimate {bytes} undercounts heap {heap}");
    let (work_tx, work_rx) = journal_work_channel();
    let budget = Arc::new(JournalBudget::new(bytes + bytes / 2));
    send_append(&work_tx, &budget, 0, entries.clone(), None).unwrap();
    assert_eq!(budget.try_reserve(bytes), Ok(false));
    let blocked = std::thread::spawn({
        let (work_tx, budget) = (work_tx.clone(), Arc::clone(&budget));
        move || send_append(&work_tx, &budget, 1, entries, None)
    });
    assert!(!blocked.is_finished());
    let JournalWork::Append { reserved, .. } = work_rx.recv().unwrap() else {
        panic!("expected append");
    };
    // The first batch is a clone (exact string capacities), so it reserves
    // what its own capacities say: at least the heap, at most the original's.
    assert!(reserved >= heap && reserved <= bytes);
    budget.release(reserved);
    blocked.join().expect("journal producer").unwrap();
}

fn entity_with(
    villager: Option<Box<crate::villager::VillagerState>>,
    projectile: Option<Box<crate::projectiles::ProjectileState>>,
) -> JournalEntry {
    JournalEntry::Entity {
        id: 1,
        uuid: String::new(),
        type_id: 0,
        health: 1.0,
        alive: true,
        age: 0,
        breed_cooldown: 0,
        pickup_delay: 0,
        item: None,
        appearance: Default::default(),
        active_effects: Default::default(),
        villager,
        state: Default::default(),
        projectile,
        tick: 0,
        pos: [0.0; 3],
        vel: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
    }
}

/// Growing nested containers are walked, not flat-priced: a scoreboard team
/// with 100k members and villagers with large-custom-data offers reserve at
/// least their real heap and block the sender once the budget is full.
#[test]
fn growing_nested_containers_reserve_their_heap_and_block_the_sender() {
    const MEMBERS: usize = 100_000;
    let mut board = crate::scoreboard::Scoreboard::default();
    let mut team = crate::scoreboard::Team {
        display_name: "t".into(),
        color: 21,
        friendly_fire: true,
        see_friendly_invisibles: true,
        nametag_visibility: 0,
        death_message_visibility: 0,
        collision_rule: 0,
        prefix: String::new(),
        suffix: String::new(),
        members: Default::default(),
    };
    team.members = (0..MEMBERS).map(|index| format!("member{index}")).collect();
    let member_heap: usize = team.members.iter().map(String::capacity).sum::<usize>()
        + MEMBERS * std::mem::size_of::<String>();
    board.teams.insert("t".into(), team);
    assert_reserves_heap_and_blocks(vec![JournalEntry::Scoreboard { value: board }], member_heap);

    const DATA: usize = 1 << 20;
    let mut result = mc_data::ItemStack::new(2, 1);
    result.components = Some(Box::new(mc_data::item_stack::ExtendedItemComponents {
        custom_data: Some(vec![0; DATA]),
        ..Default::default()
    }));
    let offer = crate::villager::VillagerTradeOffer {
        cost_a: mc_data::ItemStack::new(1, 1),
        cost_b: None,
        result,
        uses: 0,
        max_uses: 12,
        reward_exp: true,
        xp: 1,
        special_price: 0,
        demand: 0,
        price_multiplier: 0.05,
    };
    let villager = crate::villager::VillagerState {
        offers: vec![offer; 3],
        ..Default::default()
    };
    assert_reserves_heap_and_blocks(vec![entity_with(Some(Box::new(villager)), None)], 3 * DATA);
}

/// A lingering cloud's hit history is a growing map: it must move the estimate.
#[test]
fn cloud_hit_history_grows_the_estimate() {
    const HITS: usize = 50_000;
    let projectile = |hits: usize| {
        let mut state = crate::projectiles::ProjectileState::new(
            crate::projectiles::ProjectileKind::AreaEffectCloud,
            None,
            mc_data::ItemStack::EMPTY,
        );
        let mut cloud = crate::projectiles::payloads::CloudState::empty(0);
        cloud.affected_until = (0..hits as u128)
            .map(|index| (uuid::Uuid::from_u128(index), 0))
            .collect();
        state.data = crate::projectiles::ProjectileData::Cloud(cloud);
        vec![entity_with(None, Some(Box::new(state)))]
    };
    let empty = crate::journal_budget::entries_bytes(&projectile(0));
    let full = projectile(HITS);
    assert!(
        crate::journal_budget::entries_bytes(&full)
            >= empty + HITS * (std::mem::size_of::<uuid::Uuid>() + std::mem::size_of::<u64>())
    );
    assert_reserves_heap_and_blocks(full, HITS * 24);
}
