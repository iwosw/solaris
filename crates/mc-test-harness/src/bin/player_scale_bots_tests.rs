use super::*;
use bytes::BytesMut;
use mc_protocol::frame::{Compression, try_decode_frame};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn terrain_bot(x: f64, y: i16, z: f64, direction: u32) -> Bot {
    let args = Args::try_parse_from([
        "player-scale-bots",
        "--addr",
        "127.0.0.1:1",
        "--clients",
        "1",
        "--view-distance",
        "2",
        "--report",
        "/unused-report.json",
        "--no-edits",
        "--explore",
    ])
    .expect("test arguments");
    let facts = Arc::new(Facts {
        states: vec![
            StateFacts {
                passable: true,
                support: false,
            },
            StateFacts {
                passable: false,
                support: true,
            },
        ],
        dirt_item: 1,
        dirt_state: 1,
        air_state: 0,
        projectile_type_ids: [0; 2],
        grass_states: vec![1],
    });
    let mut bot = Bot::new(
        0,
        &args,
        facts,
        Arc::new(Mutex::new(Counters::default())),
        Arc::new(Mutex::new(HashSet::new())),
    );
    bot.x = x;
    bot.y = f64::from(y);
    bot.z = z;
    bot.direction = direction;
    bot.creative = true;
    let (cx, cz) = (
        (x.floor() as i32).div_euclid(16),
        (z.floor() as i32).div_euclid(16),
    );
    for sx in cx - 1..=cx + 1 {
        for sz in cz - 1..=cz + 1 {
            bot.surfaces.insert(
                (sx, sz),
                Surface {
                    x: sx,
                    z: sz,
                    columns: [Column {
                        feet_y: y,
                        top: 1,
                        below: 1,
                        walkable: true,
                    }; 256],
                    platform_cells: None,
                },
            );
        }
    }
    bot
}

async fn connection() -> Result<(Client, TcpStream, BytesMut)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let client = Client::connect(listener.local_addr()?).await?;
    let (peer, _) = listener.accept().await?;
    Ok((client, peer, BytesMut::new()))
}

async fn movement_tick(
    bot: &mut Bot,
    client: &mut Client,
    peer: &mut TcpStream,
    buffer: &mut BytesMut,
) -> Result<(PlayerInput, ServerboundMovePlayerPosRot)> {
    bot.tick(client).await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut input = None;
        let mut movement = None;
        loop {
            if let Some(mut frame) = try_decode_frame(buffer, Compression::Disabled)? {
                match frame.id {
                    ServerboundPlayerInput::ID => {
                        input = Some(ServerboundPlayerInput::decode(&mut frame.body)?.input);
                    }
                    ServerboundMovePlayerPosRot::ID => {
                        movement = Some(ServerboundMovePlayerPosRot::decode(&mut frame.body)?);
                    }
                    ServerboundClientTickEnd::ID => {
                        return Ok((
                            input.context("tick input")?,
                            movement.context("tick movement")?,
                        ));
                    }
                    _ => {}
                }
            } else {
                ensure!(
                    peer.read_buf(buffer).await? > 0,
                    "client closed before tick end"
                );
            }
        }
    })
    .await
    .context("movement packet deadline")?
}

#[test]
fn relative_position_sync_resolves_position_and_rotation() {
    let mut bot = terrain_bot(12.0, 67, -4.0, 0);
    bot.yaw = 30.0;
    bot.pitch = 40.0;
    bot.velocity_y = -0.4;
    let reply = bot.position_sync_reply(&SynchronizePlayerPosition {
        teleport_id: 7,
        x: 1.0,
        y: -2.0,
        z: 3.0,
        dx: 0.0,
        dy: 0.0,
        dz: 0.0,
        yaw: 2.0,
        pitch: -5.0,
        relative_flags: 31,
    });
    assert_eq!([reply.x, reply.y, reply.z], [13.0, 65.0, -1.0]);
    assert_eq!([reply.yaw, reply.pitch], [32.0, 35.0]);
    assert_eq!([bot.yaw, bot.pitch], [32.0, 35.0]);
    assert_eq!(bot.velocity_y, 0.0);
    assert!(!reply.flags.on_ground);
    assert!(!bot.last_on_ground);
}

#[tokio::test]
async fn teleport_confirmation_reports_pose_without_offering_movement() -> Result<()> {
    let mut bot = terrain_bot(0.5, 64, 0.5, 0);
    bot.yaw = -90.0;
    bot.pitch = 12.0;
    let packet = SynchronizePlayerPosition {
        teleport_id: 17,
        x: -61.5,
        y: 66.0,
        z: -63.5,
        dx: 0.0,
        dy: 0.0,
        dz: 0.0,
        yaw: 0.0,
        pitch: 90.0,
        relative_flags: 0,
    };
    let (mut client, mut peer, mut buffer) = connection().await?;
    let mut body = BytesMut::new();
    packet.encode(&mut body)?;
    bot.frame(
        &mut client,
        RawFrame {
            id: SynchronizePlayerPosition::ID,
            body: body.freeze(),
        },
        2,
        true,
    )
    .await?;
    let mut frames = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while frames.len() < 2 {
            if let Some(frame) = try_decode_frame(&mut buffer, Compression::Disabled)? {
                frames.push(frame);
            } else {
                ensure!(
                    peer.read_buf(&mut buffer).await? > 0,
                    "teleport reply closed"
                );
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("teleport reply deadline")??;
    assert_eq!(frames[0].id, ConfirmTeleportation::ID);
    assert_eq!(
        ConfirmTeleportation::decode(&mut frames[0].body)?.teleport_id,
        packet.teleport_id
    );
    assert_eq!(frames[1].id, ServerboundMovePlayerPosRot::ID);
    let reply = ServerboundMovePlayerPosRot::decode(&mut frames[1].body)?;
    assert_eq!([reply.x, reply.y, reply.z], [packet.x, packet.y, packet.z]);
    assert_eq!([reply.yaw, reply.pitch], [packet.yaw, packet.pitch]);
    assert_eq!([bot.yaw, bot.pitch], [packet.yaw, packet.pitch]);
    assert_eq!(bot.metrics.teleports_confirmed, 1);
    assert_eq!(bot.metrics.movements_sent, 0);
    assert!(!reply.flags.on_ground);
    Ok(())
}

async fn clears_step_before_advancing(x: f64, z: f64, direction: u32) -> Result<()> {
    let mut bot = terrain_bot(x, 63, z, direction);
    let wall = match direction {
        0 => x.floor() + 1.0,
        1 => z.floor() + 1.0,
        2 => x.floor(),
        _ => z.floor(),
    };
    for surface in bot.surfaces.values_mut() {
        for (offset, column) in surface.columns.iter_mut().enumerate() {
            let bx = surface.x * 16 + (offset % 16) as i32;
            let bz = surface.z * 16 + (offset / 16) as i32;
            let raised = match direction {
                0 => f64::from(bx) >= wall,
                1 => f64::from(bz) >= wall,
                2 => f64::from(bx) < wall,
                _ => f64::from(bz) < wall,
            };
            if raised {
                column.feet_y = 64;
            }
        }
    }
    let (mut client, mut peer, mut buffer) = connection().await?;
    let mut jumps = 0;
    let mut crossed = false;
    for tick in 0..20 {
        let (input, movement) =
            movement_tick(&mut bot, &mut client, &mut peer, &mut buffer).await?;
        jumps += u32::from(input.jump);
        if tick == 0 {
            assert_eq!((movement.x, movement.z), (x, z));
            assert!(input.jump);
            assert!(!movement.flags.on_ground);
            assert!(movement.y > 63.0);
        }
        let overlaps_step = match direction {
            0 => movement.x + 0.3 > wall,
            1 => movement.z + 0.3 > wall,
            2 => movement.x - 0.3 < wall,
            _ => movement.z - 0.3 < wall,
        };
        if overlaps_step {
            assert!(
                movement.y >= 64.0,
                "body entered the full-block step below its top"
            );
        }
        crossed |= match direction {
            0 => movement.x > wall,
            1 => movement.z > wall,
            2 => movement.x < wall,
            _ => movement.z < wall,
        };
    }
    assert!(crossed, "player never walked across the step");
    assert_eq!(jumps, 1, "cleared terrain caused repeated jump requests");
    Ok(())
}

#[tokio::test]
async fn frozen_sand_positions_clear_steps_before_horizontal_overlap() -> Result<()> {
    clears_step_before_advancing(1028.5, 1069.6224000000002, 1).await?;
    clears_step_before_advancing(1025.3769499999985, 1068.5, 2).await
}

#[tokio::test]
async fn full_body_steps_work_across_negative_and_positive_chunk_edges() -> Result<()> {
    clears_step_before_advancing(-16.3776, -0.5, 0).await?;
    clears_step_before_advancing(15.6224, 0.5, 0).await
}

#[tokio::test]
async fn missing_neighbor_forbids_horizontal_entry_without_an_invented_jump() -> Result<()> {
    let mut bot = terrain_bot(15.65, 63, 0.5, 0);
    bot.surfaces.retain(|&(cx, _), _| cx != 1);
    let (mut client, mut peer, mut buffer) = connection().await?;
    let (input, movement) = movement_tick(&mut bot, &mut client, &mut peer, &mut buffer).await?;
    assert_eq!((movement.x, movement.y, movement.z), (15.65, 63.0, 0.5));
    assert!(!input.forward);
    assert!(!input.jump);
    assert!(movement.flags.on_ground); // The observed current support remains intact.
    Ok(())
}

#[tokio::test]
async fn successive_support_removal_updates_keep_sending_airborne_descent() -> Result<()> {
    let mut bot = terrain_bot(0.5, 64, 0.5, 0);
    let (mut client, mut peer, mut buffer) = connection().await?;
    for y in [63, 62] {
        let mut body = BytesMut::new();
        BlockUpdate {
            position: pack_block_pos(0, y, 0),
            state_id: 0,
        }
        .encode(&mut body)?;
        bot.frame(
            &mut client,
            RawFrame {
                id: BlockUpdate::ID,
                body: body.freeze(),
            },
            2,
            false,
        )
        .await?;
    }
    let mut previous_y = 64.0;
    for tick in 0..10 {
        let (input, movement) =
            movement_tick(&mut bot, &mut client, &mut peer, &mut buffer).await?;
        assert_eq!((movement.x, movement.z), (0.5, 0.5));
        assert!(!movement.flags.on_ground);
        assert!(!input.jump);
        assert!(movement.y <= previous_y);
        if tick > 0 {
            assert!(
                movement.y < previous_y,
                "missing support suppressed gravity"
            );
        }
        previous_y = movement.y;
    }
    assert!(
        previous_y < 62.0,
        "unknown deeper terrain became fabricated support"
    );
    Ok(())
}

#[tokio::test]
async fn taller_neighbor_does_not_hide_intact_support_under_the_player() -> Result<()> {
    let mut bot = terrain_bot(0.8, 64, 0.5, 0);
    let (mut client, mut peer, mut buffer) = connection().await?;
    for y in [64, 65] {
        let mut body = BytesMut::new();
        BlockUpdate {
            position: pack_block_pos(1, y, 0),
            state_id: 1,
        }
        .encode(&mut body)?;
        bot.frame(
            &mut client,
            RawFrame {
                id: BlockUpdate::ID,
                body: body.freeze(),
            },
            2,
            false,
        )
        .await?;
    }
    for _ in 0..5 {
        let (input, movement) =
            movement_tick(&mut bot, &mut client, &mut peer, &mut buffer).await?;
        assert_eq!(movement.y, 64.0);
        assert!(movement.flags.on_ground);
        assert!(!input.jump, "a two-block neighbour is not a walkable step");
    }
    Ok(())
}

fn add_entity_frame(entity_id: i32, uuid: uuid::Uuid, type_id: i32) -> RawFrame {
    let mut body = BytesMut::new();
    AddEntity {
        entity_id,
        uuid,
        entity_type_id: type_id,
        x: 0.0,
        y: 64.0,
        z: 0.0,
        movement: EntityVec3::ZERO,
        pitch: 0.0,
        yaw: 0.0,
        head_yaw: 0.0,
        data: 0,
    }
    .encode(&mut body)
    .expect("encode test AddEntity");
    RawFrame {
        id: AddEntity::ID,
        body: body.freeze(),
    }
}

#[tokio::test]
async fn projectile_first_sighting_counts_once_across_restream() -> Result<()> {
    // projectile_type_ids [0;2] in terrain_bot matches type 0 below.
    let mut bot = terrain_bot(0.5, 64, 0.5, 0);
    let (mut client, _peer, _buffer) = connection().await?;
    let arrow = uuid::Uuid::from_u128(0x1234);
    for _ in 0..2 {
        bot.frame(&mut client, add_entity_frame(7, arrow, 0), 2, false)
            .await?;
    }
    assert_eq!(bot.metrics.projectiles_spawned, 1);
    assert_eq!(bot.metrics.projectiles_observed, 2);
    // A new arrow counts; the shared set survives a simulated respawn
    // re-stream of the first arrow (no clearing anywhere in frame path).
    bot.frame(
        &mut client,
        add_entity_frame(9, uuid::Uuid::from_u128(0x5678), 0),
        2,
        false,
    )
    .await?;
    bot.frame(&mut client, add_entity_frame(7, arrow, 0), 2, false)
        .await?;
    assert_eq!(bot.metrics.projectiles_spawned, 2);
    assert_eq!(bot.metrics.projectiles_observed, 4);
    Ok(())
}

#[tokio::test]
async fn projectile_shared_set_dedupes_across_bots() -> Result<()> {
    let shared = Arc::new(Mutex::new(HashSet::new()));
    let args = Args::try_parse_from([
        "player-scale-bots",
        "--addr",
        "127.0.0.1:1",
        "--clients",
        "2",
        "--view-distance",
        "2",
        "--report",
        "/unused-report.json",
        "--no-edits",
    ])
    .expect("test arguments");
    let facts = Arc::new(Facts {
        states: vec![StateFacts {
            passable: true,
            support: false,
        }],
        dirt_item: 1,
        dirt_state: 1,
        air_state: 0,
        projectile_type_ids: [0; 2],
        grass_states: vec![1],
    });
    let mut a = Bot::new(
        0,
        &args,
        Arc::clone(&facts),
        Arc::new(Mutex::new(Counters::default())),
        Arc::clone(&shared),
    );
    let mut b = Bot::new(
        1,
        &args,
        Arc::clone(&facts),
        Arc::new(Mutex::new(Counters::default())),
        Arc::clone(&shared),
    );
    let (mut client, _peer, _buffer) = connection().await?;
    let arrow = uuid::Uuid::from_u128(0x9abc);
    a.frame(&mut client, add_entity_frame(7, arrow, 0), 2, false)
        .await?;
    b.frame(&mut client, add_entity_frame(7, arrow, 0), 2, false)
        .await?;
    assert_eq!(
        a.metrics.projectiles_spawned + b.metrics.projectiles_spawned,
        1
    );
    assert_eq!(
        a.metrics.projectiles_observed + b.metrics.projectiles_observed,
        2
    );
    Ok(())
}

#[tokio::test]
async fn initial_play_accepts_both_center_position_orders() -> Result<()> {
    for center_first in [true, false] {
        let mut bot = terrain_bot(0.5, 64, 0.5, 0);
        let (mut client, mut peer, _buffer) = connection().await?;
        let mut center_body = BytesMut::new();
        SetCenterChunk {
            chunk_x: 0,
            chunk_z: 0,
        }
        .encode(&mut center_body)?;
        let mut position_body = BytesMut::new();
        SynchronizePlayerPosition {
            teleport_id: 1,
            x: 0.5,
            y: 64.0,
            z: 0.5,
            dx: 0.0,
            dy: 0.0,
            dz: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            relative_flags: 0,
        }
        .encode(&mut position_body)?;
        let center = mc_protocol::frame::encode_frame(
            SetCenterChunk::ID,
            &center_body,
            Compression::Disabled,
        )?;
        let position = mc_protocol::frame::encode_frame(
            SynchronizePlayerPosition::ID,
            &position_body,
            Compression::Disabled,
        )?;
        let frames = if center_first {
            [&center, &position]
        } else {
            [&position, &center]
        };
        for frame in frames {
            peer.write_all(frame).await?;
        }
        tokio::time::timeout(Duration::from_secs(5), bot.initial_play(&mut client, 2))
            .await
            .context("both initial packet orders must complete")??;
        assert_eq!(bot.metrics.teleports_confirmed, 1);
        assert_eq!(bot.metrics.packets_received, 2);
    }
    Ok(())
}

fn configure_open_loop(bot: &mut Bot, windows: Value, elapsed: Duration) {
    let schedule: OpenLoopSchedule =
        serde_json::from_value(json!({"windows": windows})).expect("test schedule");
    schedule.validate().expect("valid test schedule");
    bot.open_loop = true;
    bot.schedule = Arc::new(schedule);
    bot.metrics.windows = vec![WindowCounters::default(); bot.schedule.windows.len()];
    bot.accounted_slots = vec![[0; 4]; bot.schedule.windows.len()];
    *bot.epoch.lock().unwrap_or_else(|error| error.into_inner()) = Some(RunEpoch {
        instant: Instant::now() - elapsed,
        unix_ms: 42,
        unix_ns: 42_000_000,
    });
}

fn system_chat_frame(text: &str) -> Result<RawFrame> {
    component_chat_frame(&mc_nbt::Tag::Compound(vec![(
        "text".into(),
        mc_nbt::Tag::String(text.into()),
    )]))
}

fn component_chat_frame(tag: &mc_nbt::Tag) -> Result<RawFrame> {
    let mut content_nbt = Vec::new();
    mc_nbt::write_network(&mut content_nbt, tag)?;
    let mut body = BytesMut::new();
    ClientboundSystemChat {
        content_nbt,
        overlay: false,
    }
    .encode(&mut body)?;
    Ok(RawFrame {
        id: ClientboundSystemChat::ID,
        body: body.freeze(),
    })
}

#[test]
fn fixture_save_success_requires_exact_public_reply_grammar() {
    for text in [
        "Saved the game",
        "Saved 0 players, 0 entities, 0 chunks",
        "Saved 1 players, 2 entities, 5 chunks",
        "Saved 1000 players, 100064 entities, 4096 chunks",
    ] {
        assert!(fixture_save_succeeded(text), "{text}");
    }
    for text in [
        "Save-all failed; see server log",
        "Saved the game!",
        "<scale_0> Saved the game",
        "Saved 1 players, 2 entities, 5 chunks trailing",
        "Saved 1 players, 2 entities, 5 chunks\n",
        "Saved 1 players, 2 entities, chunks",
        "Saved players, 2 entities, 5 chunks",
        "Saved +1 players, 2 entities, 5 chunks",
        "Saved 1 players, -2 entities, 5 chunks",
        "Saved 1 players, 2 entities, 5.0 chunks",
        "Saved 1 players, 2 entities, 18446744073709551616 chunks",
        "Saved 1 players, 2 entities",
    ] {
        assert!(!fixture_save_succeeded(text), "{text}");
    }
}

#[tokio::test]
async fn public_save_feedback_accepts_success_and_refuses_failure() -> Result<()> {
    let mut bot = terrain_bot(0.5, 64, 0.5, 0);
    let (mut client, _peer, _buffer) = connection().await?;
    let component = |text: &str| {
        component_chat_frame(&mc_nbt::Tag::Compound(vec![(
            "text".into(),
            mc_nbt::Tag::String(text.into()),
        )]))
    };
    bot.frame(
        &mut client,
        component("Saved 1 players, 2 entities, 5 chunks")?,
        2,
        false,
    )
    .await?;
    assert_eq!(bot.metrics.command_feedback_received, 1);
    let error = bot
        .frame(
            &mut client,
            component("Save-all failed; see server log")?,
            2,
            false,
        )
        .await
        .expect_err("a failed save must refuse fixture preparation");
    assert!(
        error
            .to_string()
            .contains("fixture command refused: Save-all failed; see server log")
    );
    assert_eq!(bot.metrics.command_feedback_received, 1);
    Ok(())
}

#[test]
fn open_loop_is_opt_in_and_control_is_strict() {
    let args = Args::try_parse_from([
        "bots",
        "--addr",
        "127.0.0.1:1",
        "--clients",
        "4096",
        "--view-distance",
        "2",
        "--report",
        "/unused.json",
    ])
    .unwrap();
    assert!(!args.open_loop);
    assert!(!args.unassisted_login);
    assert!(!args.verify_platform);
    assert!(serde_json::from_value::<RunControl>(json!({"event":"run"})).is_ok());
    assert!(serde_json::from_value::<RunControl>(json!({"event":"stop"})).is_ok());
    assert!(serde_json::from_value::<RunControl>(json!({"event":"run","extra":1})).is_err());
    assert!(serde_json::from_value::<RunControl>(json!({"event":"stop","extra":1})).is_err());
    assert!(serde_json::from_value::<RunControl>(json!({"event":"pause"})).is_err());
}

#[test]
fn prepare_concurrency_defaults_to_three_and_rejects_zero() {
    let base = [
        "bots",
        "--addr",
        "127.0.0.1:1",
        "--clients",
        "4",
        "--view-distance",
        "2",
        "--report",
        "/unused.json",
    ];
    assert_eq!(Args::try_parse_from(base).unwrap().prepare_concurrency, 3);
    let with = |value: &'static str| {
        Args::try_parse_from(base.into_iter().chain(["--prepare-concurrency", value]))
    };
    assert_eq!(with("32").unwrap().prepare_concurrency, 32);
    assert!(with("0").is_err());
}

#[test]
fn wall_slots_include_misses_without_replaying_them() {
    let mut accounted = 0;
    assert_eq!(
        due_slot(Duration::from_millis(1266), 20, &mut accounted),
        Some((25, 16_000))
    );
    assert_eq!(accounted, 25);
    assert_eq!(
        due_slot(Duration::from_millis(1266), 20, &mut accounted),
        None
    );
    assert_eq!(
        due_slot(Duration::from_millis(1301), 20, &mut accounted),
        Some((26, 1_000))
    );
    assert_eq!(elapsed_slots(Duration::from_secs(2), 20), 40);
    assert_eq!(elapsed_slots(Duration::from_secs(2), 0), 0);
    assert_eq!(
        due_slot(Duration::from_nanos(333_333_334), 3, &mut 0),
        Some((1, 0))
    );
}

#[test]
fn schedule_rejects_unbounded_or_recovery_demand() {
    for windows in [
        json!([]),
        json!([{"name":"a","duration_ms":0}]),
        json!([{"name":"a","duration_ms":1,"movement_hz":1001}]),
        json!([{"name":"a","duration_ms":1,"command_hz":1}]),
        json!([{"name":"a","duration_ms":1,"commands":["bad\ncommand"]}]),
        json!([{"name":"a","duration_ms":1,"recovery":true,"chat_hz":1}]),
        json!([{"name":"a","duration_ms":1},{"name":"a","duration_ms":2}]),
    ] {
        let schedule: OpenLoopSchedule =
            serde_json::from_value(json!({"windows":windows})).unwrap();
        assert!(schedule.validate().is_err());
    }
    assert!(
        serde_json::from_value::<OpenLoopSchedule>(json!({
            "windows":[{"name":"a","duration_ms":1,"unknown":1}]
        }))
        .is_err()
    );
    let schedule: OpenLoopSchedule = serde_json::from_value(json!({"windows":[
        {"name":"steady","duration_ms":1000,"movement_hz":20},
        {"name":"burst","duration_ms":500,"movement_hz":40},
        {"name":"recovery","duration_ms":1000,"recovery":true}
    ]}))
    .unwrap();
    schedule.validate().unwrap();
    assert_eq!(schedule.window_at(Duration::from_millis(999)), Some(0));
    assert_eq!(schedule.window_at(Duration::from_millis(1000)), Some(1));
    assert_eq!(schedule.window_at(Duration::from_millis(1500)), Some(2));
    assert_eq!(schedule.window_at(Duration::from_millis(2500)), None);
}

#[test]
fn aggregate_offered_is_wall_based_even_with_absent_bot_snapshots() -> Result<()> {
    let mut bot = terrain_bot(0.5, 64, 0.5, 0);
    configure_open_loop(
        &mut bot,
        json!([
            {"name":"steady","duration_ms":1000,"movement_hz":20,"edit_hz":2,"chat_hz":1},
            {"name":"burst","duration_ms":1000,"movement_hz":40},
            {"name":"recovery","duration_ms":1000,"recovery":true}
        ]),
        Duration::from_millis(2500),
    );
    let mut args = Args::try_parse_from([
        "bots",
        "--addr",
        "127.0.0.1:1",
        "--clients",
        "4096",
        "--view-distance",
        "2",
        "--report",
        "/unused.json",
        "--open-loop",
        "--schedule-file",
        "/unused-schedule.json",
    ])?;
    args.schedule = Arc::clone(&bot.schedule);
    args.epoch = Arc::clone(&bot.epoch);
    *args.ended.lock().unwrap_or_else(|error| error.into_inner()) =
        Some(Duration::from_millis(2500));
    let report = aggregate(&[], &args, 4096, Instant::now(), "sample", None)?;
    assert_eq!(
        report["open_loop"]["windows"][0]["movement"]["offered"],
        20 * 4096
    );
    assert_eq!(
        report["open_loop"]["windows"][0]["movement"]["dropped_late"],
        20 * 4096
    );
    assert_eq!(
        report["open_loop"]["windows"][1]["movement"]["offered"],
        40 * 4096
    );
    assert_eq!(report["open_loop"]["windows"][2]["movement"]["offered"], 0);
    assert_eq!(
        report["open_loop"]["windows"][0]["movement"]["completed"],
        0
    );
    assert_eq!(report["open_loop"]["epoch_unix_ns"], 42_000_000);
    let slots = SlotCounters {
        admitted: 3,
        dropped_capacity: 2,
        dropped_unavailable: 1,
        ..SlotCounters::default()
    };
    assert_eq!(slots.json(10)["dropped_late"], 4);
    assert_eq!(slots.json(10)["dropped"], 7);
    Ok(())
}

#[tokio::test]
async fn pre_run_recovery_and_finished_windows_never_issue_gameplay() -> Result<()> {
    let mut bot = terrain_bot(0.5, 64, 0.5, 0);
    configure_open_loop(
        &mut bot,
        json!([
            {"name":"steady","duration_ms":1000,"movement_hz":20,"edit_hz":2,"chat_hz":1},
            {"name":"recovery","duration_ms":2000,"recovery":true}
        ]),
        Duration::from_millis(1500),
    );
    bot.no_edits = false;
    let (mut client, mut peer, mut buffer) = connection().await?;
    let epoch = *bot.epoch.lock().unwrap_or_else(|error| error.into_inner());
    *bot.epoch.lock().unwrap_or_else(|error| error.into_inner()) = None;
    bot.open_loop_poll(&mut client).await?;
    *bot.epoch.lock().unwrap_or_else(|error| error.into_inner()) = epoch;
    bot.open_loop_poll(&mut client).await?;
    bot.epoch
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_mut()
        .unwrap()
        .instant = Instant::now() - Duration::from_secs(4);
    bot.open_loop_poll(&mut client).await?;
    assert_eq!(bot.metrics.movements_sent, 0);
    assert_eq!(bot.metrics.breaks_sent + bot.metrics.places_sent, 0);
    assert_eq!(bot.metrics.chats_sent + bot.metrics.commands_sent, 0);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), peer.read_buf(&mut buffer))
            .await
            .is_err()
    );
    // Protocol replies continue while gameplay is suppressed.
    let mut body = BytesMut::new();
    ClientboundKeepAlive { id: 123 }.encode(&mut body)?;
    bot.frame(
        &mut client,
        RawFrame {
            id: ClientboundKeepAlive::ID,
            body: body.freeze(),
        },
        2,
        false,
    )
    .await?;
    tokio::time::timeout(Duration::from_secs(1), peer.read_buf(&mut buffer)).await??;
    let mut frame =
        try_decode_frame(&mut buffer, Compression::Disabled)?.context("keepalive reply")?;
    assert_eq!(frame.id, ServerboundKeepAlive::ID);
    assert_eq!(ServerboundKeepAlive::decode(&mut frame.body)?.id, 123);
    assert_eq!(bot.metrics.keepalives_replied, 1);
    Ok(())
}

#[tokio::test]
async fn edit_capacity_rejects_latest_offer_instead_of_throttling_schedule() -> Result<()> {
    let mut bot = terrain_bot(0.5, 64, 0.5, 0);
    configure_open_loop(
        &mut bot,
        json!([
            {"name":"steady","duration_ms":10000,"edit_hz":20}
        ]),
        Duration::from_millis(500),
    );
    bot.no_edits = false;
    for sequence in 0..ACTION_CAP {
        bot.acks.push_back(PendingAck {
            sequence: sequence as i32,
            sent: Instant::now(),
            edit: Edit::Break,
        });
    }
    let (mut client, _peer, _buffer) = connection().await?;
    bot.open_loop_poll(&mut client).await?;
    assert!(bot.accounted_slots[0][1] >= 10);
    assert_eq!(bot.metrics.windows[0].edit.dropped_capacity, 1);
    assert_eq!(bot.metrics.windows[0].edit.admitted, 0);
    assert_eq!(bot.metrics.breaks_sent, 0);
    assert_eq!(bot.acks.len(), ACTION_CAP);
    Ok(())
}

#[tokio::test]
async fn open_loop_edits_do_not_wait_for_previous_observations() -> Result<()> {
    let mut bot = terrain_bot(0.5, 64, 0.5, 0);
    configure_open_loop(
        &mut bot,
        json!([
            {"name":"steady","duration_ms":10000,"edit_hz":20}
        ]),
        Duration::from_millis(500),
    );
    bot.no_edits = false;
    let (mut client, _peer, _buffer) = connection().await?;
    assert!(bot.edit(&mut client).await?);
    assert!(bot.edit(&mut client).await?);
    assert_eq!(bot.acks.len(), 2);
    assert_eq!(bot.observations.len(), 2);
    assert_eq!(bot.metrics.break_updates_observed, 0);
    assert_ne!(bot.observations[0].position, bot.observations[1].position);
    let position = bot.observations[0].position;
    bot.update(position, 0, "test_update", Some("steady"));
    bot.update(position, 0, "duplicate_test_update", Some("steady"));
    assert_eq!(bot.metrics.windows[0].edit.completed, 1);
    assert_eq!(bot.metrics.break_updates_observed, 1);
    Ok(())
}

#[tokio::test]
async fn chat_completion_requires_exact_own_echo_and_counts_once_in_recovery() -> Result<()> {
    let mut bot = terrain_bot(0.5, 64, 0.5, 0);
    configure_open_loop(
        &mut bot,
        json!([
            {"name":"steady","duration_ms":1000,"chat_hz":2},
            {"name":"recovery","duration_ms":2000,"recovery":true}
        ]),
        Duration::from_millis(500),
    );
    let (mut client, _peer, _buffer) = connection().await?;
    bot.open_loop_poll(&mut client).await?;
    assert_eq!(bot.metrics.windows[0].chat.admitted, 1);
    assert_eq!(bot.metrics.windows[0].chat.completed, 0);
    let expected = bot
        .pending_chats
        .front()
        .context("pending echo")?
        .expected
        .clone();
    bot.epoch
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_mut()
        .unwrap()
        .instant = Instant::now() - Duration::from_millis(1500);
    bot.frame(
        &mut client,
        system_chat_frame("another player's chat")?,
        2,
        false,
    )
    .await?;
    assert_eq!(bot.metrics.chat_echoes_received, 0);
    let username = format!("<{}>", bot.username);
    let suffix = expected
        .strip_prefix(&username)
        .context("own echo prefix")?;
    let structured = mc_nbt::Tag::Compound(vec![
        ("text".into(), mc_nbt::Tag::String("<".into())),
        (
            "extra".into(),
            mc_nbt::Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: vec![
                    mc_nbt::Tag::Compound(vec![(
                        "text".into(),
                        mc_nbt::Tag::String(bot.username.clone()),
                    )]),
                    mc_nbt::Tag::Compound(vec![(
                        "text".into(),
                        mc_nbt::Tag::String(format!(">{suffix}")),
                    )]),
                ],
            }),
        ),
    ]);
    bot.frame(&mut client, component_chat_frame(&structured)?, 2, false)
        .await?;
    bot.frame(&mut client, system_chat_frame(&expected)?, 2, false)
        .await?;
    assert_eq!(bot.metrics.chat_echoes_received, 1);
    assert_eq!(bot.metrics.windows[0].chat.completed, 1);
    assert_eq!(bot.metrics.windows[1].received.chat_echoes_received, 1);
    assert_eq!(bot.metrics.windows[1].chat.admitted, 0);
    Ok(())
}

#[test]
fn literal_chat_match_rejects_partial_and_dynamic_components() {
    use mc_nbt::Tag;
    let component = Tag::Compound(vec![("text".into(), Tag::String("<own> message".into()))]);
    assert!(literal_component_matches(&component, "<own> message"));
    assert!(!literal_component_matches(
        &component,
        "<own> message extra"
    ));
    assert!(!literal_component_matches(&component, "<other> message"));
    let translated = Tag::Compound(vec![
        ("text".into(), Tag::String("<own> message".into())),
        ("translate".into(), Tag::String("chat.type.text".into())),
    ]);
    assert!(!literal_component_matches(&translated, "<own> message"));
}

#[test]
fn platform_proof_requires_all_five_real_grass_and_clearance_columns() -> Result<()> {
    let mut bot = terrain_bot(0.5, 64, 0.5, 0);
    assert!(bot.verify_platform().is_err());
    let columns = bot.surfaces.get(&(0, 0)).unwrap().columns;
    bot.surfaces.get_mut(&(0, 0)).unwrap().platform_cells = Some([1, 0, 0]);
    bot.surfaces.get_mut(&(0, 0)).unwrap().columns[0].feet_y = 43;
    bot.surfaces.get_mut(&(0, 0)).unwrap().columns[0].walkable = false;
    for (x, z) in [(-2, -2), (-2, 2), (2, -2), (2, 2)] {
        bot.surfaces.insert(
            (x, z),
            Surface {
                x,
                z,
                columns,
                platform_cells: Some([1, 0, 0]),
            },
        );
    }
    bot.verify_platform()?;
    assert_eq!(bot.metrics.fixture_platform_checks, 1);
    bot.surfaces.get_mut(&(2, 2)).unwrap().platform_cells = Some([1, 1, 0]);
    assert!(bot.verify_platform().is_err());
    assert_eq!(bot.metrics.fixture_platform_checks, 0);
    Ok(())
}

#[test]
fn platform_cells_decode_from_sections_not_advertised_heightmaps() -> Result<()> {
    use mc_protocol::codec::WriteMc;

    let bot = terrain_bot(0.5, 64, 0.5, 0);
    let mut body = Vec::new();
    WriteMc::write_i32(&mut body, 0);
    WriteMc::write_i32(&mut body, 0);
    body.write_varint(1);
    body.write_varint(5);
    let mut heights = [0_u64; 37];
    for index in 0..256 {
        heights[index / 7] |= 107_u64 << (index % 7 * 9);
    }
    body.write_varint(i32::try_from(heights.len())?);
    for word in heights {
        body.extend_from_slice(&word.to_be_bytes());
    }
    let mut sections = Vec::new();
    for section in 0..HEIGHT / 16 {
        let state = i32::from(section == 7);
        WriteMc::write_i16(&mut sections, if state == 1 { 4096 } else { 0 });
        WriteMc::write_i16(&mut sections, 0);
        WriteMc::write_u8(&mut sections, 0);
        sections.write_varint(state);
        WriteMc::write_u8(&mut sections, 0);
        sections.write_varint(0);
    }
    body.write_varint(i32::try_from(sections.len())?);
    body.extend_from_slice(&sections);
    let surface = read_surface(&body, &bot.facts)?;
    assert!(surface.columns.iter().all(|column| column.feet_y == 64
        && column.top == 1
        && column.below == 1
        && column.walkable));
    assert_eq!(surface.platform_cells, Some([1, 0, 0]));
    Ok(())
}

#[tokio::test]
async fn all_login_tasks_wait_for_one_producer_epoch_after_rendezvous() -> Result<()> {
    let clients = 4;
    let gate = Arc::new(Barrier::new(clients + 1));
    let (release, receiver) = watch::channel(None);
    let (admitted, mut admissions) = mpsc::channel(clients);
    let mut tasks = JoinSet::new();
    for _ in 0..clients {
        let gate = Arc::clone(&gate);
        let mut receiver = receiver.clone();
        let admitted = admitted.clone();
        tasks.spawn(async move {
            let unix_ns = wait_for_login_release(&gate, &mut receiver).await?;
            admitted
                .send(unix_ns)
                .await
                .context("record login admission")
        });
    }
    drop(admitted);
    tokio::time::timeout(Duration::from_secs(5), gate.wait()).await?;
    assert!(matches!(
        admissions.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    release.send_replace(Some(123_456_789));
    for _ in 0..clients {
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), admissions.recv()).await?,
            Some(123_456_789),
        );
    }
    while let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(())
}

#[test]
fn street_routes_reject_shortcuts_and_wrong_client_count() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("routes.json");
    std::fs::write(&path, "[[[0,64,0],[1,64,0],[1,64,1],[0,64,1]]]")?;
    assert_eq!(StreetRoutes::load(&path, 1)?.0.len(), 1);
    assert!(StreetRoutes::load(&path, 2).is_err());
    std::fs::write(&path, "[[[0,64,0],[2,64,0]]]")?;
    assert!(StreetRoutes::load(&path, 1).is_err());
    std::fs::write(&path, "[[[0,64,0],[1,66,0]]]")?;
    assert!(StreetRoutes::load(&path, 1).is_err());
    Ok(())
}

#[tokio::test]
async fn street_routes_walk_received_ground_and_turn_without_teleporting() -> Result<()> {
    let mut bot = terrain_bot(0.5, 64, 0.5, 0);
    bot.street_routes = Arc::new(StreetRoutes(vec![vec![
        [0, 64, 0],
        [1, 64, 0],
        [1, 64, 1],
        [0, 64, 1],
    ]]));
    bot.street_waypoint = 1;
    assert_eq!(bot.metrics.street_waypoints_reached, 0);
    let (mut client, mut peer, mut buffer) = connection().await?;
    for _ in 0..12 {
        movement_tick(&mut bot, &mut client, &mut peer, &mut buffer).await?;
        assert!((0.5..=1.5).contains(&bot.x));
        assert!((0.5..=1.5).contains(&bot.z));
    }
    assert!(bot.metrics.walking_distance_blocks > 2.0);
    assert!(bot.street_waypoint >= 2);
    assert_eq!(bot.metrics.street_waypoints_reached, 2);
    assert_eq!(bot.metrics.route_refusals_missing_column, 0);
    Ok(())
}

#[tokio::test]
async fn street_routes_off_axis_correction_stops_input_and_records_refusal() -> Result<()> {
    let mut bot = terrain_bot(0.75, 64, 0.75, 0);
    bot.street_routes = Arc::new(StreetRoutes(vec![vec![[0, 64, 0], [1, 64, 0]]]));
    bot.street_waypoint = 1;
    let (mut client, mut peer, mut buffer) = connection().await?;
    let (input, _) = movement_tick(&mut bot, &mut client, &mut peer, &mut buffer).await?;
    assert!(!input.forward);
    assert_eq!(bot.metrics.walking_distance_blocks, 0.0);
    assert_eq!(bot.metrics.route_refusals_not_walkable, 1);
    assert_eq!(bot.metrics.street_waypoints_reached, 0);
    Ok(())
}

include!("player_scale_bots_mechanic_tests.rs");
