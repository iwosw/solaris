use std::sync::Arc;

use super::*;
use crate::boundary::{ConnectionId, PlayerInput};
use crate::journal::{
    CheckpointManifest, JournalEntry, JournalWriter, Replay, read_complete_batches_after,
    replay_batches,
};
use crate::native_checkpoint::tests::{read_manifest, write_manifest};
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;

fn fixture() -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 1, 512)
}

fn weather_tick(state: &mut TickState) {
    let previous = view(state);
    state.tick += 1;
    advance_cycle(state);
    fade_and_publish(state, previous);
}

#[test]
fn weather_levels_follow_old_f32_steps_and_thunder_fades_independently() {
    let mut state = fixture();
    set_command(&mut state, Weather::Rain, Some(5_000));
    assert_eq!((state.rain_level, state.thunder_level), (0.0, 0.0));
    let mut expected = 0.0_f32;
    for _ in 0..100 {
        weather_tick(&mut state);
        expected = (expected + 0.01).min(1.0);
        assert_eq!(state.rain_level.to_bits(), expected.to_bits());
        assert_eq!(state.thunder_level, 0.0);
    }
    assert!(state.rain_level >= 0.99);
    // The old oracle records 101 updates: f32 addition reaches the clamp
    // one tick after the nominal 100-tick ramp. Do not round away its bits.
    weather_tick(&mut state);
    assert_eq!(state.rain_level, 1.0);

    set_command(&mut state, Weather::Thunder, Some(5_000));
    assert_eq!(state.thunder_level, 0.0);
    weather_tick(&mut state);
    assert_eq!(state.thunder_level, 0.01);
    assert_eq!(state.rain_level, 1.0);
    for _ in 0..100 {
        weather_tick(&mut state);
    }
    assert_eq!(state.thunder_level, 1.0);

    set_command(&mut state, Weather::Rain, Some(5_000));
    assert_eq!(state.thunder_level, 1.0);
    weather_tick(&mut state);
    assert_eq!(state.rain_level, 1.0);
    assert_eq!(state.thunder_level, 1.0 - 0.01);
    for _ in 0..100 {
        weather_tick(&mut state);
    }
    assert_eq!(state.thunder_level, 0.0);
    assert_eq!(state.rain_level, 1.0);

    set_command(&mut state, Weather::Clear, Some(5_000));
    assert_eq!(state.rain_level, 1.0);
    for _ in 0..101 {
        weather_tick(&mut state);
    }
    assert_eq!((state.rain_level, state.thunder_level), (0.0, 0.0));
}

#[test]
fn frozen_cycle_retains_command_duration_but_both_levels_still_fade() {
    let mut state = fixture();
    state
        .gamerules
        .insert("advance_weather".into(), "false".into());
    state.rain_level = 0.37;
    state.thunder_level = 0.12;
    set_command(&mut state, Weather::Thunder, Some(3));
    assert_eq!((state.rain_level, state.thunder_level), (0.37, 0.12));
    for _ in 0..101 {
        weather_tick(&mut state);
        assert_eq!(state.weather, (Weather::Thunder, 3));
    }
    assert_eq!((state.rain_level, state.thunder_level), (1.0, 1.0));
    state
        .gamerules
        .insert("advance_weather".into(), "true".into());
    weather_tick(&mut state);
    assert_eq!(state.weather, (Weather::Thunder, 2));
    weather_tick(&mut state);
    assert_eq!(state.weather, (Weather::Thunder, 1));
    weather_tick(&mut state);
    assert_eq!(state.weather.0, Weather::Clear);
    assert!((12_000..=180_000).contains(&state.weather.1));
    assert_eq!((state.rain_level, state.thunder_level), (0.99, 0.99));
}

#[test]
fn natural_cycle_only_alternates_old_clear_and_rain_targets() {
    let mut state = fixture();
    for (initial, next, range) in [
        (Weather::Clear, Weather::Rain, 12_000..=24_000),
        (Weather::Rain, Weather::Clear, 12_000..=180_000),
        (Weather::Thunder, Weather::Clear, 12_000..=180_000),
    ] {
        state.weather = (initial, 1);
        weather_tick(&mut state);
        assert_eq!(state.weather.0, next);
        assert!(range.contains(&state.weather.1));
    }
}

#[test]
fn tick_snapshots_and_reliable_changes_use_real_levels_with_a_frozen_day_clock() {
    let mut state = fixture();
    state
        .gamerules
        .insert("advance_time".into(), "false".into());
    state
        .gamerules
        .insert("advance_weather".into(), "false".into());
    state.day_time = 87_654;
    let connection = ConnectionId(31);
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection,
            uuid: uuid::Uuid::new_v4(),
            name: "weather".into(),
            permission_level: 0,
        }],
    );
    let (output, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Command {
            connection,
            command: "weather thunder 5s".into(),
            permission_level: 4,
        }],
    );
    assert_eq!(state.weather, (Weather::Thunder, 100));
    assert_eq!(output.snapshot.day_time, 87_654);
    assert_eq!(output.snapshot.day_time_rate, 0.0);
    assert!(output.snapshot.raining);
    assert_eq!(output.snapshot.rain_level, 0.01);
    assert_eq!(output.snapshot.thunder_level, 0.01);
    let weather: Vec<_> = drains
        .iter()
        .filter(|(recipient, _)| *recipient == connection)
        .flat_map(|(_, changes)| changes)
        .filter(|change| matches!(change, PlayerChange::WeatherChanged { .. }))
        .collect();
    assert_eq!(weather.len(), 1, "command and fade share one publication");
    assert!(matches!(weather[0], PlayerChange::WeatherChanged {
        raining: true, rain_level, thunder_level,
    } if *rain_level == 0.01 && *thunder_level == 0.01));
    let (output, _, _) = run_tick(&mut state, &[]);
    assert_eq!(output.snapshot.rain_level, 0.02);
    assert_eq!(output.snapshot.thunder_level, 0.02);
    assert_eq!(output.snapshot.day_time, 87_654);
    assert_eq!(state.weather, (Weather::Thunder, 100));
}

#[test]
fn checkpoint_and_tail_fold_restore_exact_clock_duration_and_float_bits() {
    let dir = std::env::temp_dir().join(format!("mc-core-weather-fold-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    let config = crate::CoreConfig::new(dir.clone(), 42);
    let mut state = fixture();
    state.day_time = 93_211;
    state.weather = (Weather::Thunder, 432);
    state.rain_level = 0.37123457;
    state.thunder_level = 0.12876543;
    state
        .gamerules
        .insert("advance_time".into(), "false".into());
    state
        .gamerules
        .insert("advance_weather".into(), "false".into());
    let base_image = ClimateImage::capture(&state);
    let mut writer = JournalWriter::open(&dir, 1).expect("writer");
    writer
        .append(1, 1, &[JournalEntry::Climate { value: base_image }])
        .expect("base WAL");
    writer.gate(0, 1).expect("base gate");
    write_manifest(
        &config,
        &CheckpointManifest {
            tick: 1,
            players: Default::default(),
            world_spawn: Some([0, 80, 0]),
            world_spawn_rotation: [0.0; 2],
            generated_entity_claims: Default::default(),
            block_ticks: Default::default(),
            climate: Some(base_image),
            ..Default::default()
        },
    )
    .expect("checkpoint");
    let checkpoint = read_manifest(&config).unwrap();
    let mut restored = fixture();
    checkpoint
        .climate
        .unwrap()
        .restore(&mut restored)
        .expect("restore base");
    assert_eq!(ClimateImage::capture(&restored), base_image);

    state.day_time = 111_777;
    state.weather = (Weather::Clear, 77);
    state.rain_level = 0.2456789;
    state.thunder_level = 0.91234565;
    let tail = ClimateImage::capture(&state);
    writer
        .append(2, 2, &[JournalEntry::Climate { value: tail }])
        .expect("tail WAL");
    writer.flush().expect("flush");
    let batches = read_complete_batches_after(&dir.join("journal"), checkpoint.tick).unwrap();
    let replay = replay_batches(
        Replay {
            climate: checkpoint.climate,
            ..Default::default()
        },
        &batches,
    );
    assert_eq!(replay.tick, 2);
    replay
        .climate
        .unwrap()
        .restore(&mut restored)
        .expect("restore tail");
    assert_eq!(ClimateImage::capture(&restored), tail);
    assert_eq!(restored.journaled_climate, Some(tail));
    let (output, _, _) = run_tick(&mut restored, &[]);
    assert_eq!(output.snapshot.day_time, tail.day_time);
    assert_eq!(restored.weather, (Weather::Clear, 77));
    assert_eq!(
        output.snapshot.rain_level.to_bits(),
        (state.rain_level - 0.01).to_bits()
    );
    assert_eq!(
        output.snapshot.thunder_level.to_bits(),
        (state.thunder_level - 0.01).to_bits()
    );
    drop(writer);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn real_core_reopen_uses_checkpoint_and_tail_climate_not_the_tick_number() {
    for with_tail in [false, true] {
        let dir =
            std::env::temp_dir().join(format!("mc-core-weather-open-{}", uuid::Uuid::new_v4()));
        let mut config = crate::config::CoreConfig::new(dir.clone(), 42);
        config.simulation_distance = 1;
        config.checkpoint_every_ticks = u64::MAX;
        crate::core::Core::open(config.clone())
            .expect("initial core")
            .shutdown();
        let mut manifest = read_manifest(&config).unwrap();
        let image = ClimateImage {
            day_time: 123_987,
            weather_kind: 2,
            remaining_ticks: 1_234,
            rain_level_bits: 0.37123457_f32.to_bits(),
            thunder_level_bits: 0.12876543_f32.to_bits(),
            advance_time: false,
            advance_weather: false,
        };
        let recovered_tick = if with_tail {
            let tick = manifest.tick + 1;
            let mut writer = JournalWriter::open(&dir, 1).expect("writer");
            writer
                .append(tick, tick, &[JournalEntry::Climate { value: image }])
                .expect("tail");
            writer.flush().expect("tail flush");
            tick
        } else {
            manifest.climate = Some(image);
            write_manifest(&config, &manifest).expect("climate checkpoint");
            manifest.tick
        };
        let core = crate::core::Core::open(config.clone()).expect("reopened core");
        let mut rx = core.subscribe();
        let output = rx.blocking_recv().expect("recovered tick");
        assert_eq!(output.snapshot.day_time, image.day_time);
        assert_eq!(output.snapshot.day_time_rate, 0.0);
        assert!(output.snapshot.raining);
        let mut rain = f32::from_bits(image.rain_level_bits);
        let mut thunder = f32::from_bits(image.thunder_level_bits);
        for _ in recovered_tick..output.tick {
            rain = step_level(rain, true);
            thunder = step_level(thunder, true);
        }
        assert_eq!(output.snapshot.rain_level.to_bits(), rain.to_bits());
        assert_eq!(output.snapshot.thunder_level.to_bits(), thunder.to_bits());
        core.shutdown();
        let final_manifest = read_manifest(&config).unwrap();
        let final_image = final_manifest.climate.expect("durable climate");
        assert_eq!(final_image.day_time, image.day_time);
        assert_eq!(final_image.remaining_ticks, image.remaining_ticks);
        assert!(!final_image.advance_time && !final_image.advance_weather);
        let mut rain = f32::from_bits(image.rain_level_bits);
        let mut thunder = f32::from_bits(image.thunder_level_bits);
        for _ in recovered_tick..final_manifest.tick {
            rain = step_level(rain, true);
            thunder = step_level(thunder, true);
        }
        assert_eq!(final_image.rain_level_bits, rain.to_bits());
        assert_eq!(final_image.thunder_level_bits, thunder.to_bits());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}

#[test]
fn invalid_persisted_weather_fails_without_partially_restoring_the_clock() {
    let mut state = fixture();
    state.day_time = 77;
    let valid = ClimateImage::capture(&state);
    for invalid in [
        ClimateImage {
            weather_kind: 3,
            ..valid
        },
        ClimateImage {
            remaining_ticks: -1,
            ..valid
        },
        ClimateImage {
            rain_level_bits: f32::NAN.to_bits(),
            ..valid
        },
        ClimateImage {
            thunder_level_bits: 1.1_f32.to_bits(),
            ..valid
        },
    ] {
        assert!(invalid.restore(&mut state).is_err());
        assert_eq!(ClimateImage::capture(&state), valid);
    }
}

#[test]
fn frozen_saturated_climate_does_not_rejournal_unchanged_images() {
    let mut state = fixture();
    state.weather = (Weather::Rain, 200);
    state.rain_level = 1.0;
    state
        .gamerules
        .insert("advance_time".into(), "false".into());
    state
        .gamerules
        .insert("advance_weather".into(), "false".into());
    let (output, _, _) = run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, JournalEntry::Climate { .. }))
            .count(),
        1
    );
    let (output, _, _) = run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, JournalEntry::Climate { .. }))
    );
}
