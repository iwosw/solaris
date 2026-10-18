//! Real redb checkpoint + tick-WAL reopen: policy and physical money together.
use super::*;
use crate::CoreConfig;
use crate::dimensions::Dimension;
use crate::journal::JournalWriter;
use crate::settlements::{caravan, economy, persistence};

fn native(f: &mut Fixture, request: Request) {
    economy::execute(
        &mut f.state,
        f.settlement,
        economy::Request::Commerce(request),
    )
    .unwrap();
}

fn disk_fixture(config: &CoreConfig) -> Fixture {
    // Unlike the small domain fixtures, this world really owns native storage.
    let mut state = crate::core::recover_dimension_state(config, Dimension::Overworld).unwrap();
    // Tick zero is the database's creation commit, never a second checkpoint.
    let _ = crate::tick::run_tick(&mut state, &[]);
    fixture_in(state)
}

fn checkpoint(f: &mut Fixture, config: &CoreConfig) {
    use crate::native_checkpoint::{encode_entities, encode_players, record, refresh_metadata};
    use mc_world::native::NativeCheckpoint;
    let store = crate::native_checkpoint::tests::fixture_store(config).unwrap();
    let tick = f.state.tick;
    let (players, captured, block_entities) = crate::core::checkpoint_data_for(&f.state, tick);
    let entities = encode_entities(
        Dimension::Overworld,
        &captured,
        &f.state.registry.items,
        tick,
    )
    .unwrap();
    let players = encode_players(players.iter(), &f.state.registry.items, tick).unwrap();
    let capture = f.state.world.capture_dirty(&block_entities);
    let written = capture.written();
    let mut image = store.read_checkpoint().unwrap();
    if image.tick == 0 {
        assert!(
            !written.is_empty(),
            "the baseline must persist the actual chests/floor"
        );
    }
    let mut manifest: crate::journal::CheckpointManifest =
        serde_json::from_slice(&image.globals["checkpoint"].payload).unwrap();
    manifest.tick = tick;
    for child in manifest.dimensions.values_mut() {
        child.tick = tick;
    }
    image
        .globals
        .extend(persistence::checkpoint_rows(&f.state, tick).unwrap());
    image.globals.insert(
        "plugins".into(),
        record(tick, serde_json::to_vec(&f.state.plugins).unwrap()),
    );
    image.globals.insert(
        "checkpoint".into(),
        record(tick, serde_json::to_vec(&manifest).unwrap()),
    );
    refresh_metadata(&mut image.globals, &manifest).unwrap();
    for global in image.globals.values_mut() {
        global.captured_tick = tick;
    }
    store
        .commit_checkpoint_stream(
            NativeCheckpoint {
                tick,
                previous_tick: image.tick,
                chunks: &[],
                entities: &[],
                players: &players,
                globals: &image.globals,
            },
            |write| {
                capture
                    .visit_native(|snapshot| {
                        write(snapshot)
                            .map_err(|error| crate::config::CoreError::Storage(error.to_string()))
                    })
                    .map_err(|error| mc_world::native::NativeError::Source(error.to_string()))
            },
            entities.iter().map(Ok),
            &f.state.registry.blocks,
            &f.state.registry.items,
        )
        .unwrap();
    f.state.world.mark_flushed(&written);
}

fn step(f: &mut Fixture, writer: &mut JournalWriter) {
    let (output, _, _) = crate::tick::run_tick(&mut f.state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut f.state, &output);
    writer.append(output.tick, output.tick, &entries).unwrap();
}

fn through(f: &mut Fixture, writer: &mut JournalWriter, tick: u64) {
    while f.state.tick <= tick {
        step(f, writer);
    }
}

fn reopen(f: Fixture, config: &CoreConfig, mut writer: JournalWriter) -> Fixture {
    writer.flush().unwrap();
    drop(writer);
    let Fixture {
        state,
        settlement,
        treasury,
        escrow,
        worker,
        worker_uuid,
        buyer,
        buyer_uuid,
        emerald,
        bread,
    } = f;
    // Close the last old world's store Arc, not merely its policy registry.
    drop(state);
    let mut state = crate::core::recover_dimension_state(config, Dimension::Overworld).unwrap();
    ensure(&mut state);
    assert_eq!(
        state.entities.slot_of_uuid(worker_uuid),
        state.entities.slot_of(worker)
    );
    assert_eq!(
        state.entities.slot_of_uuid(buyer_uuid),
        state.entities.slot_of(buyer)
    );
    assert!(state.entities.slot_of(worker).is_some());
    Fixture {
        state,
        settlement,
        treasury,
        escrow,
        worker,
        worker_uuid,
        buyer,
        buyer_uuid,
        emerald,
        bread,
    }
}

#[test]
fn automatic_wages_survive_actual_checkpoint_and_wal_reopen_without_pay_commands() {
    for save_checkpoint in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let config = CoreConfig::new(directory.path().to_path_buf(), 42);
        let mut f = disk_fixture(&config);
        let hired_at = f.state.tick;
        stock(&mut f.state, TREASURY_CHEST, f.emerald, 6);
        let request = employ(&f, "1", 2, 5, 6);
        native(&mut f, request);
        checkpoint(&mut f, &config);
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        through(&mut f, &mut writer, hired_at + 10);
        assert_eq!(carry_total(&f.state, f.worker), 4);
        if save_checkpoint {
            checkpoint(&mut f, &config);
        }
        let mut f = reopen(f, &config, writer);
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        step(&mut f, &mut writer); // Real chunk admission hydrates the chests.
        assert_eq!(carry_total(&f.state, f.worker), 4);
        assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 2);
        through(&mut f, &mut writer, hired_at + 15);
        for _ in 0..3 {
            step(&mut f, &mut writer);
        }
        let contract = &f
            .state
            .settlements
            .get(f.settlement)
            .unwrap()
            .economy
            .commerce
            .employments["1"];
        assert_eq!(
            (contract.earned, contract.paid, contract.attempt),
            (6, 6, 3)
        );
        assert_eq!(carry_total(&f.state, f.worker), 6);
        assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 0);
        writer.flush().unwrap();
    }
}

#[test]
fn stopped_debt_survives_actual_reopen_and_refill_pays_only_what_was_earned() {
    for save_checkpoint in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let config = CoreConfig::new(directory.path().to_path_buf(), 42);
        let mut f = disk_fixture(&config);
        let hired_at = f.state.tick;
        let request = employ(&f, "1", 3, 5, 12);
        native(&mut f, request);
        checkpoint(&mut f, &config);
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        through(&mut f, &mut writer, hired_at + 5);
        native(
            &mut f,
            Request::StopEmployment {
                contract: "1".into(),
            },
        );
        step(&mut f, &mut writer); // Persist the stop alongside the real owed balance.
        if save_checkpoint {
            checkpoint(&mut f, &config);
        }
        let mut f = reopen(f, &config, writer);
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        step(&mut f, &mut writer);
        assert_eq!(carry_total(&f.state, f.worker), 0);
        stock(&mut f.state, TREASURY_CHEST, f.emerald, 3);
        through(&mut f, &mut writer, hired_at + 20);
        let commerce = &f
            .state
            .settlements
            .get(f.settlement)
            .unwrap()
            .economy
            .commerce;
        let contract = &commerce.employments["1"];
        assert_eq!(
            (contract.active, contract.earned, contract.paid),
            (false, 3, 3)
        );
        assert!(!super::super::active(commerce));
        assert_eq!(carry_total(&f.state, f.worker), 3);
        assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 0);
        let mut f = reopen(f, &config, writer);
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        through(&mut f, &mut writer, 30);
        assert_eq!(carry_total(&f.state, f.worker), 3);
        assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 0);
        native(
            &mut f,
            Request::DismissEmployment {
                contract: "1".into(),
            },
        );
        assert_eq!(
            f.state
                .settlements
                .get(f.settlement)
                .unwrap()
                .economy
                .commerce
                .ended_employment,
            1
        );
        writer.flush().unwrap();
    }
}

#[test]
fn real_delivery_automatically_pays_escrow_exactly_once_across_disk_reopen() {
    for save_checkpoint in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let config = CoreConfig::new(directory.path().to_path_buf(), 42);
        let mut f = disk_fixture(&config);
        stock(&mut f.state, TREASURY_CHEST, f.emerald, 9);
        f.state
            .block_entities
            .get_mut(&(10, 201, 10))
            .unwrap()
            .slots[1] = ItemStack::new(f.bread, 1);
        let reserve = Request::EscrowReserve {
            id: "1".into(),
            shipment: "paid-native-walk".into(),
            payer: f.treasury.clone(),
            escrow: f.escrow.clone(),
            payee: carry(f.buyer_uuid),
            price: 9,
            payer_fence: fences(&f, &f.treasury),
        };
        native(&mut f, reserve);
        let buildings = &f
            .state
            .settlements
            .get(f.settlement)
            .unwrap()
            .district
            .as_ref()
            .unwrap()
            .buildings;
        let request = caravan::Request::Open(Box::new(caravan::Open {
            id: "paid-native-walk".into(),
            source: caravan::ContainerRef {
                building: buildings[0].clone(),
                dimension: f.state.dimension.name().into(),
                position: TREASURY_CHEST,
            },
            destination: caravan::ContainerRef {
                building: buildings[1].clone(),
                dimension: f.state.dimension.name().into(),
                position: ESCROW_CHEST,
            },
            carrier: f.worker_uuid,
            manifest: ItemStack::new(f.bread, 1),
            quantity: 1,
            waypoints: vec![TREASURY_DOOR, ESCROW_DOOR],
            max_ticks: 72_000,
            paid_mandate: Some("1".into()),
        }));
        economy::execute(
            &mut f.state,
            f.settlement,
            economy::Request::Caravan(request),
        )
        .unwrap();
        checkpoint(&mut f, &config);
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        let start = f
            .state
            .entities
            .pos(f.state.entities.slot_of(f.worker).unwrap());
        for _ in 0..2_000 {
            step(&mut f, &mut writer);
            if f.state
                .settlements
                .get(f.settlement)
                .unwrap()
                .economy
                .commerce
                .mandates
                .is_empty()
            {
                break;
            }
        }
        let economy = &f.state.settlements.get(f.settlement).unwrap().economy;
        assert!(
            caravan::delivered(&economy.caravan, "paid-native-walk"),
            "{:?}",
            economy.caravan
        );
        assert!(economy.commerce.mandates.is_empty());
        assert_eq!(economy.commerce.settled_mandate, 1);
        assert_eq!(carry_total(&f.state, f.buyer), 9);
        assert_ne!(
            f.state
                .entities
                .pos(f.state.entities.slot_of(f.worker).unwrap()),
            start
        );
        if save_checkpoint {
            checkpoint(&mut f, &config);
        }
        let mut f = reopen(f, &config, writer);
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        for _ in 0..20 {
            step(&mut f, &mut writer);
        }
        let economy = &f.state.settlements.get(f.settlement).unwrap().economy;
        assert!(caravan::delivered(&economy.caravan, "paid-native-walk"));
        assert!(economy.commerce.mandates.is_empty());
        assert_eq!(economy.commerce.settled_mandate, 1);
        assert_eq!(carry_total(&f.state, f.buyer), 9);
        assert_eq!(carry_total(&f.state, f.worker), 0);
        assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 0);
        assert_eq!(emerald_total(&f.state, ESCROW_CHEST), 0);
        let destination = &f.state.block_entities[&(20, 201, 10)].slots;
        assert_eq!(
            destination
                .iter()
                .filter(|s| s.item_id == f.bread)
                .map(|s| s.count)
                .sum::<i32>(),
            1
        );
        writer.flush().unwrap();
    }
}
