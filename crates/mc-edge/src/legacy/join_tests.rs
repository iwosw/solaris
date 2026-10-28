//! End to end: a raw 26.1.2 (protocol 775) client joins the real edge server
//! over TCP, and a 26.3-only block reaches it as its polyfill, both as a block
//! update and inside a chunk. Needs the canonical cache at `data/vanilla` and
//! the 26.1.2 cache beside it (`data/vanilla-26.1.2`).

use std::sync::Arc;
use std::time::Duration;

use bytes::BytesMut;
use mc_protocol::codec::{ReadMc, WriteMc};
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{RegistryData, UpdateTags};
use mc_protocol::{Compression, RawFrame};
use tokio::net::{TcpListener, TcpStream};

use super::Profile;
use crate::server::EdgeServer;

const DEADLINE: Duration = Duration::from_secs(240);
const MIN_Y: i32 = -64;

fn server(root: &std::path::Path) -> EdgeServer {
    let vanilla = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let mut config = mc_core::CoreConfig::new(root.join("world"), 42);
    config.vanilla_data_dir = Some(vanilla.clone());
    config.view_distance = 2;
    config.simulation_distance = 2;
    let core = Arc::new(mc_core::Core::open(config).unwrap());
    let mut cfg = crate::config::EdgeConfig {
        server_dir: root.join("server"),
        ..Default::default()
    };
    std::fs::create_dir_all(&cfg.server_dir).unwrap();
    cfg.data.world_dir = root.join("world");
    cfg.data.vanilla_data_dir = vanilla;
    cfg.server.view_distance = 2;
    cfg.server.simulation_distance = 2;
    cfg.auth.online_mode = false;
    let data = crate::server::load_vanilla_data(&cfg).unwrap();
    let tags = crate::server::load_tags(&cfg, &data).unwrap();
    EdgeServer::new(core, data, tags, cfg).unwrap()
}

/// Bare framing client: no translation, raw 26.1.2 ids.
struct Client {
    stream: TcpStream,
    buf: BytesMut,
    compression: Compression,
    profile: Arc<Profile>,
}

impl Client {
    async fn send(&mut self, id: i32, body: &[u8]) {
        use tokio::io::AsyncWriteExt;

        let frame = mc_protocol::frame::encode_frame(id, body, self.compression).unwrap();
        self.stream.write_all(&frame).await.unwrap();
    }

    async fn recv(&mut self) -> RawFrame {
        use tokio::io::AsyncReadExt;

        loop {
            if let Some(frame) =
                mc_protocol::frame::try_decode_frame(&mut self.buf, self.compression).unwrap()
            {
                return frame;
            }
            assert_ne!(
                self.stream.read_buf(&mut self.buf).await.unwrap(),
                0,
                "server closed"
            );
        }
    }

    fn id(&self, phase: &str, clientbound: bool, name: &str) -> i32 {
        let packets = &self.profile.tables.packets;
        let names = match (phase, clientbound) {
            ("login", true) => &packets.login_cb,
            ("login", false) => &packets.login_sb,
            ("configuration", true) => &packets.config_cb,
            ("configuration", false) => &packets.config_sb,
            (_, true) => &packets.play_cb,
            (_, false) => &packets.play_sb,
        };
        names
            .legacy_id(name)
            .unwrap_or_else(|| panic!("26.1.2 has {name}"))
    }

    /// Handshake 775 -> Login -> Configuration -> Play.
    async fn join(
        address: std::net::SocketAddr,
        name: &str,
        profile: Arc<Profile>,
        known_pack: bool,
    ) -> Self {
        let mut client = Self {
            stream: TcpStream::connect(address).await.unwrap(),
            buf: BytesMut::new(),
            compression: Compression::Disabled,
            profile,
        };
        let mut hello = Vec::new();
        hello.write_varint(super::PROTOCOL);
        hello.write_string("127.0.0.1", 255).unwrap();
        hello.write_u16(address.port());
        hello.write_varint(2);
        client.send(0, &hello).await;
        let mut start = Vec::new();
        start.write_string(name, 16).unwrap();
        start.write_uuid(crate::crypto::offline_uuid(name));
        client
            .send(client.id("login", false, "minecraft:hello"), &start)
            .await;
        loop {
            let frame = client.recv().await;
            if frame.id == client.id("login", true, "minecraft:login_compression") {
                let threshold = (&frame.body[..]).read_varint().unwrap();
                client.compression = Compression::Threshold(threshold as usize);
            } else if frame.id == client.id("login", true, "minecraft:login_finished") {
                let mut r = &frame.body[..];
                assert_eq!(r.read_uuid().unwrap(), crate::crypto::offline_uuid(name));
                assert_eq!(r.read_string(16).unwrap(), name);
                assert_eq!(r.read_varint().unwrap(), 0, "no profile properties");
                assert!(r.is_empty(), "26.1.2 profile has no session UUID");
                break;
            } else {
                panic!("unexpected login packet {:#04x}", frame.id);
            }
        }
        client
            .send(
                client.id("login", false, "minecraft:login_acknowledged"),
                &[],
            )
            .await;
        loop {
            let frame = client.recv().await;
            if frame.id == client.id("configuration", true, "minecraft:select_known_packs") {
                let mut r = &frame.body[..];
                assert_eq!(r.read_varint().unwrap(), 1);
                let pack: Vec<String> = (0..3).map(|_| r.read_string(64).unwrap()).collect();
                assert_eq!(pack, ["minecraft", "core", super::RELEASE]);
                client
                    .send(
                        client.id("configuration", false, "minecraft:select_known_packs"),
                        if known_pack { &frame.body } else { &[0] },
                    )
                    .await;
            } else if frame.id == client.id("configuration", true, "minecraft:registry_data") {
                let packet = RegistryData::decode(&mut frame.body.as_ref()).unwrap();
                let expected = client
                    .profile
                    .data
                    .registry(packet.registry_id.as_str())
                    .unwrap();
                assert_eq!(packet.entries.len(), expected.entries.len());
                for (entry, name) in packet.entries.iter().zip(&expected.entries) {
                    assert_eq!(&entry.name, name);
                    if known_pack {
                        assert!(entry.nbt_payload.is_none());
                    } else {
                        assert_eq!(
                            entry.nbt_payload.as_deref(),
                            client
                                .profile
                                .data
                                .registry_entry_payload(&packet.registry_id, name)
                                .as_deref()
                        );
                        assert!(entry.nbt_payload.is_some(), "full 26.1.2 registry payload");
                    }
                }
            } else if frame.id == client.id("configuration", true, "minecraft:update_tags") {
                let packet = UpdateTags::decode(&mut frame.body.as_ref()).unwrap();
                for registry in packet.registries {
                    let expected = &client.profile.tags.registries[&registry.registry];
                    for tag in registry.tags {
                        assert_eq!(
                            tag.entries, expected[&tag.tag],
                            "legacy tag numeric identities"
                        );
                    }
                }
            } else if frame.id == client.id("configuration", true, "minecraft:finish_configuration")
            {
                client
                    .send(
                        client.id("configuration", false, "minecraft:finish_configuration"),
                        &[],
                    )
                    .await;
                return client;
            } else if frame.id == client.id("configuration", true, "minecraft:disconnect") {
                panic!("configuration refused");
            }
        }
    }

    /// Play until `stop` returns a value; answers teleports, chunk batches
    /// and keep-alives like a vanilla client.
    async fn play_until<T>(&mut self, mut stop: impl FnMut(&Self, &RawFrame) -> Option<T>) -> T {
        loop {
            let frame = self.recv().await;
            if frame.id == self.id("play", true, "minecraft:update_advancements") {
                validate_legacy_advancement_icons(&self.profile.tables, &frame.body);
            }
            if frame.id == self.id("play", true, "minecraft:player_position") {
                let id = (&frame.body[..]).read_varint().unwrap();
                let mut accept = Vec::new();
                accept.write_varint(id);
                self.send(
                    self.id("play", false, "minecraft:accept_teleportation"),
                    &accept,
                )
                .await;
            } else if frame.id == self.id("play", true, "minecraft:chunk_batch_finished") {
                let mut rate = Vec::new();
                rate.write_f32(25.0);
                self.send(
                    self.id("play", false, "minecraft:chunk_batch_received"),
                    &rate,
                )
                .await;
            } else if frame.id == self.id("play", true, "minecraft:keep_alive") {
                self.send(self.id("play", false, "minecraft:keep_alive"), &frame.body)
                    .await;
            } else if frame.id == self.id("play", true, "minecraft:disconnect") {
                panic!("disconnected in play");
            }
            if let Some(value) = stop(self, &frame) {
                return value;
            }
        }
    }
}

/// Block state at `pos` in a 26.1.2 `level_chunk_with_light` body.
fn chunk_block(body: &[u8], pos: [i32; 3], global_bits: u8) -> Option<i32> {
    let mut r = body;
    let (x, z) = (r.read_i32().unwrap(), r.read_i32().unwrap());
    if (x, z) != (pos[0] >> 4, pos[2] >> 4) {
        return None;
    }
    for _ in 0..r.read_varint().unwrap() {
        r.read_varint().unwrap();
        let longs = r.read_varint().unwrap() as usize;
        r = &r[longs * 8..];
    }
    let data = r.read_byte_array(1 << 21).unwrap();
    let mut r = &data[..];
    let target = (pos[1] - MIN_Y) >> 4;
    let index = (((pos[1] & 15) * 16 + (pos[2] & 15)) * 16 + (pos[0] & 15)) as usize;
    for section in 0.. {
        r = &r[4..];
        let (block, rest) = container(r, 4096, 8, global_bits, Some(index));
        let (_, rest) = container(rest, 64, 3, 7, None);
        if section == target {
            return block;
        }
        r = rest;
    }
    unreachable!()
}

fn container(
    mut r: &[u8],
    volume: usize,
    max_indirect: u8,
    global_bits: u8,
    index: Option<usize>,
) -> (Option<i32>, &[u8]) {
    let bits = r.read_u8().unwrap();
    if bits == 0 {
        return (Some(r.read_varint().unwrap()), r);
    }
    let palette: Vec<i32> = if bits <= max_indirect {
        let n = r.read_varint().unwrap();
        (0..n).map(|_| r.read_varint().unwrap()).collect()
    } else {
        assert_eq!(
            bits, global_bits,
            "global palette at the client's own width"
        );
        Vec::new()
    };
    let per = 64 / usize::from(bits);
    let words = volume.div_ceil(per);
    let value = index.map(|index| {
        let mut long = &r[index / per * 8..];
        let raw =
            (long.read_u64().unwrap() >> (index % per * usize::from(bits))) & ((1 << bits) - 1);
        if palette.is_empty() {
            raw as i32
        } else {
            palette[raw as usize]
        }
    });
    (value, &r[words * 8..])
}

/// Rebuild the canonical envelope from the old layout, walking every legacy
/// icon through its real component schemas. The canonical decoder then rejects
/// duplicate/contradictory component patches instead of accepting a bare frame.
fn validate_legacy_advancement_icons(tables: &super::tables::Tables, body: &[u8]) {
    use mc_protocol::packets::play::ClientboundUpdateAdvancements;
    let mut input = body;
    let mut canonical = Vec::new();
    canonical.write_bool(input.read_bool().unwrap());
    let added = input.read_varint().unwrap();
    canonical.write_varint(added);
    for _ in 0..added {
        let id = input.read_string(32767).unwrap();
        canonical.write_string(&id, 32767).unwrap();
        let parent = input.read_bool().unwrap();
        canonical.write_bool(parent);
        if parent {
            canonical
                .write_string(&input.read_string(32767).unwrap(), 32767)
                .unwrap();
        }
        let display = input.read_bool().unwrap();
        canonical.write_bool(display);
        let mut coordinates = [0.0; 2];
        if display {
            super::items::copy_nbt(&mut input, &mut canonical).unwrap();
            super::items::copy_nbt(&mut input, &mut canonical).unwrap();
            super::items::template(
                tables,
                super::items::To::Canonical,
                &mut input,
                &mut canonical,
            )
            .unwrap_or_else(|error| panic!("advancement {id} legacy icon: {error:#}"));
            canonical.write_varint(input.read_varint().unwrap());
            let flags = input.read_i32().unwrap();
            canonical.write_i32(flags);
            if flags & 1 != 0 {
                canonical
                    .write_string(&input.read_string(32767).unwrap(), 32767)
                    .unwrap();
            }
            coordinates = [input.read_f32().unwrap(), input.read_f32().unwrap()];
        }
        let requirements = input.read_varint().unwrap();
        canonical.write_varint(requirements);
        for _ in 0..requirements {
            let count = input.read_varint().unwrap();
            canonical.write_varint(count);
            for _ in 0..count {
                canonical
                    .write_string(&input.read_string(32767).unwrap(), 32767)
                    .unwrap();
            }
        }
        canonical.write_bool(input.read_bool().unwrap());
        for coordinate in coordinates {
            canonical.write_f32(coordinate);
        }
    }
    canonical.extend_from_slice(input);
    let mut remaining = canonical.as_slice();
    ClientboundUpdateAdvancements::decode(&mut remaining)
        .expect("legacy advancement icons have valid, unique component patches");
    assert!(
        remaining.is_empty(),
        "legacy advancement envelope fully consumed"
    );
}

#[ignore = "requires local 26.3 and 26.1.2 content caches"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_client_joins_and_sees_polyfilled_chunk() {
    assert_eq!(
        mc_protocol::PROTOCOL_VERSION,
        777,
        "canonical server is 26.3"
    );
    let dir = tempfile::tempdir().unwrap();
    let server = server(dir.path());
    let profile = server
        .legacy
        .clone()
        .expect("26.1.2 content cache beside data/vanilla (data/vanilla-26.1.2)");
    let hub = Arc::clone(&server.hub);
    let tables = &profile.tables;
    let cinnabar = server
        .hub
        .registry()
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:cinnabar").unwrap())
        .expect("26.3 server knows cinnabar")
        .default
        .0;
    let legacy_blocks = mc_data::blocks::load_blocks_report(
        profile
            .data
            .sidecar_root()
            .unwrap()
            .join("reports/blocks.json"),
    )
    .unwrap();
    let granite = legacy_blocks
        .iter()
        .find(|block| block.id.as_str() == "minecraft:granite")
        .unwrap()
        .states
        .iter()
        .find(|state| state.default)
        .unwrap()
        .id as i32;
    assert_eq!(tables.block_states.get(cinnabar as i32), Some(granite));
    assert_ne!(granite, cinnabar as i32, "the polyfill changes the id");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let serve = tokio::spawn(crate::server::serve(listener, server));

    tokio::time::timeout(DEADLINE, async {
        let mut first = Client::join(address, "Legacy1", Arc::clone(&profile), true).await;
        let spawn = first
            .play_until(|client, frame| {
                (frame.id == client.id("play", true, "minecraft:player_position")).then(|| {
                    let mut r = &frame.body[..];
                    r.read_varint().unwrap();
                    [
                        r.read_f64().unwrap(),
                        r.read_f64().unwrap(),
                        r.read_f64().unwrap(),
                    ]
                })
            })
            .await;
        let pos = [
            spawn[0].floor() as i32,
            spawn[1].floor() as i32 + 3,
            spawn[2].floor() as i32,
        ];
        let bits = tables.legacy_block_bits;
        first
            .play_until(|client, frame| {
                (frame.id == client.id("play", true, "minecraft:level_chunk_with_light"))
                    .then(|| chunk_block(&frame.body, pos, bits))
                    .flatten()
            })
            .await;
        let (command, done) = mc_core::command_console::ConsoleCommand::new(format!(
            "setblock {} {} {} minecraft:cinnabar",
            pos[0], pos[1], pos[2]
        ));
        assert!(hub.submit(mc_core::boundary::PlayerInput::ConsoleCommand(command)));
        done.await.unwrap();
        let packed = mc_protocol::packets::play::pack_block_pos(pos[0], pos[1], pos[2]);
        let updated = first
            .play_until(|client, frame| {
                let mut r = &frame.body[..];
                (frame.id == client.id("play", true, "minecraft:block_update")
                    && r.read_i64().unwrap() == packed)
                    .then(|| r.read_varint().unwrap())
            })
            .await;
        assert_eq!(updated, granite, "block update carries the polyfill");
        drop(first);

        let mut second = Client::join(address, "Legacy2", Arc::clone(&profile), false).await;
        let seen = second
            .play_until(|client, frame| {
                (frame.id == client.id("play", true, "minecraft:level_chunk_with_light"))
                    .then(|| chunk_block(&frame.body, pos, bits))
                    .flatten()
            })
            .await;
        assert_eq!(
            seen, granite,
            "the chunk carries the polyfill, not the 26.3 id"
        );
    })
    .await
    .expect("legacy join and chunk within the deadline");
    hub.shutdown();
    serve.abort();
}
