use super::*;

fn request(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut bytes = vec![1, kind, 0, 0, 0, 7];
    bytes.extend_from_slice(body);
    bytes
}

#[test]
fn computer_wire_roundtrip_and_exact_bounds() {
    let at = [-17i32, 64, 25];
    let coords: Vec<u8> = at.into_iter().flat_map(i32::to_be_bytes).collect();
    let bytes = request(0, &coords);
    let packet = ServerboundCustomPayload {
        payload: CustomPayload::Unknown {
            channel: Identifier::parse(CHANNEL).unwrap(),
            payload: bytes.clone(),
        },
    };
    let mut wire = Vec::new();
    packet.encode(&mut wire).unwrap();
    assert_eq!(
        decode(&payload(&wire).unwrap().unwrap()).unwrap(),
        Request::Open { session: 7, at }
    );
    assert_eq!(
        decode(&request(1, "hello λ".as_bytes())).unwrap(),
        Request::Input {
            session: 7,
            text: "hello λ".into()
        }
    );
    assert_eq!(
        decode(&request(2, &[])).unwrap(),
        Request::Close { session: 7 }
    );
    assert!(decode(&request(1, &vec![b'a'; MAX_INPUT])).is_ok());
    for malformed in [
        request(0, &[]),
        request(2, &[0]),
        request(3, &[]),
        request(1, b"a\nb"),
        request(1, &[0xff]),
        request(1, &vec![b'a'; MAX_INPUT + 1]),
        vec![1, 2, 0, 0, 0, 0],
        vec![2, 2, 0, 0, 0, 1],
    ] {
        assert!(decode(&malformed).is_err(), "{malformed:?}");
    }
}

#[test]
fn computer_permission_matrix_fails_closed() {
    let mut cfg = ComputerConfig::default();
    assert!(!permitted(&cfg, true, true, 4, false));
    cfg.enabled = true;
    assert!(permitted(&cfg, true, false, 4, false));
    assert!(!permitted(&cfg, false, true, 4, false));
    assert!(!permitted(&cfg, true, true, 3, false));
    assert!(!permitted(&cfg, true, true, 4, true));
    cfg.local_only = false;
    assert!(!permitted(&cfg, false, false, 4, false));
    assert!(permitted(&cfg, false, true, 4, false));
}

#[test]
fn computer_config_is_opt_in_and_requires_explicit_process_paths() {
    let cfg = crate::config::EdgeConfig::from_toml("[computer]\nenabled = true\n", None).unwrap();
    assert!(cfg.computer.enabled);
    assert!(cfg.computer.local_only);
    assert!(cfg.computer.command.is_empty());
    let mut process = cfg.computer;
    process.command = vec!["sh".into()];
    assert!(process.validate().is_err());
    process.command = vec!["/bin/cat".into()];
    assert!(process.validate().is_err());
    process.working_directory = PathBuf::from("/tmp");
    assert!(process.validate().is_ok());
}

#[cfg(unix)]
#[tokio::test]
async fn computer_bridge_keeps_session_and_passes_input_literally() {
    let cfg = ComputerConfig {
        enabled: true,
        command: vec!["/bin/cat".into()],
        working_directory: PathBuf::from("/tmp"),
        ..ComputerConfig::default()
    };
    let mut child = spawn(&cfg).unwrap().unwrap();
    for text in ["hello agent\n", "$(touch /not-a-command); λ\n"] {
        child.input.write_all(text.as_bytes()).await.unwrap();
        let mut output = vec![0; text.len()];
        tokio::time::timeout(Duration::from_secs(2), child.output.read_exact(&mut output))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(output, text.as_bytes());
    }
    child._child.start_kill().unwrap();
    tokio::time::timeout(Duration::from_secs(2), child._child.wait())
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn computer_target_requires_live_player_loaded_carrier_and_reach() {
    use mc_core::boundary::{
        ChangeSet, EntityAppearance, EntityView, SnapshotChunk, WorldSnapshot,
    };
    use mc_world::{
        BlockStateId,
        chunk::{Chunk, ChunkPos},
    };
    let at = [-17, 64, 25];
    let pos = ChunkPos { x: -2, z: 1 };
    let mut chunk = Chunk::empty(
        pos,
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk.set_block(15, 64, 9, BlockStateId(42));
    let player = EntityView {
        id: EntityId(7),
        uuid: uuid::Uuid::nil(),
        type_id: 1,
        position: [-17.0, 64.0, 25.0],
        velocity: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        head_yaw: 0.0,
        on_ground: true,
        appearance: Arc::new(EntityAppearance {
            health: 20.0,
            ..Default::default()
        }),
    };
    let make = |player: EntityView, loaded: bool| TickOutput {
        tick: 1,
        snapshot: Arc::new(WorldSnapshot {
            dimension: Dimension::Nether,
            chunks: Arc::new(if loaded {
                vec![SnapshotChunk {
                    pos,
                    revision: 1,
                    chunk: Arc::new(chunk.clone()),
                }]
            } else {
                vec![]
            }),
            entities: vec![player].into(),
            ..Default::default()
        }),
        changes: Arc::new(ChangeSet::default()),
        times: Default::default(),
    };
    let output = make(player.clone(), true);
    assert_eq!(
        target_dimension(&output, EntityId(7), at, 42),
        Some(Dimension::Nether)
    );
    assert_eq!(target_dimension(&output, EntityId(8), at, 42), None);
    assert_eq!(target_dimension(&output, EntityId(7), at, 43), None);
    assert_eq!(
        target_dimension(&make(player.clone(), false), EntityId(7), at, 42),
        None
    );
    let mut far = player.clone();
    far.position[0] += 10.0;
    assert_eq!(
        target_dimension(&make(far, true), EntityId(7), at, 42),
        None
    );
    let mut dead = player.clone();
    Arc::make_mut(&mut dead.appearance).health = 0.0;
    assert_eq!(
        target_dimension(&make(dead, true), EntityId(7), at, 42),
        None
    );
    let mut invalid = player;
    invalid.position[0] = f64::NAN;
    assert_eq!(
        target_dimension(&make(invalid, true), EntityId(7), at, 42),
        None
    );
}
