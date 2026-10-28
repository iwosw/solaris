//! quirk-probe: vanilla player-quirk wire bot (attribute-swap, diagonal-speed).
//!
//! Walks Handshake -> Login (offline) -> Configuration -> Play exactly like a
//! vanilla client (same prelude as `wire-probe`), then runs one scenario and
//! prints ONE JSON object of clientbound-only observations to stdout. Full
//! detail goes to `--out`.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use bytes::BytesMut;
use clap::Parser;
use mc_protocol::PROTOCOL_VERSION;
use mc_protocol::RawFrame;
use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, ServerboundKnownPacks,
};
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_protocol::packets::login::{LoginAcknowledged, LoginStart, LoginSuccess};
use mc_protocol::packets::play::{
    AddEntity, ClientboundContainerSetContent, ClientboundContainerSetSlot, ClientboundDamageEvent,
    ClientboundKeepAlive, ClientboundSetEntityData, ClientboundSetHealth, ConfirmTeleportation,
    EntityDataValue, LoginPlay, MoveEntityPosRot, MovePlayerFlags, PlayerInput, RemoveEntities,
    ServerboundAttack, ServerboundClientTickEnd, ServerboundKeepAlive, ServerboundMovePlayerPosRot,
    ServerboundPlayerInput, ServerboundPlayerLoaded, ServerboundPunch, ServerboundSetCarriedItem,
    SetEntityMotion, SynchronizePlayerPosition,
};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(name = "quirk-probe", about = "Vanilla player-quirk wire probe")]
struct Cli {
    #[arg(long)]
    addr: String,
    #[arg(long, default_value = "probe")]
    name: String,
    #[arg(long, value_parser = ["attribute-swap", "diagonal-speed"])]
    scenario: String,
    #[arg(long)]
    out: PathBuf,
}

struct Probe {
    stream: TcpStream,
    rbuf: BytesMut,
    compression: Compression,
}

impl Probe {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            rbuf: BytesMut::with_capacity(8192),
            compression: Compression::Disabled,
        }
    }

    async fn write_packet<P: Packet>(&mut self, packet: &P) -> Result<()> {
        let mut body = BytesMut::new();
        packet.encode(&mut body)?;
        let framed = encode_frame(P::ID, &body, self.compression)?;
        self.stream.write_all(&framed).await?;
        Ok(())
    }

    async fn read_frame(&mut self) -> Result<RawFrame> {
        loop {
            if let Some(frame) = try_decode_frame(&mut self.rbuf, self.compression)? {
                return Ok(frame);
            }
            let n = self.stream.read_buf(&mut self.rbuf).await?;
            if n == 0 {
                bail!("peer closed");
            }
        }
    }

    async fn drain(&mut self, dur: Duration, seen: &mut Vec<RawFrame>) -> Result<()> {
        let deadline = tokio::time::Instant::now() + dur;
        loop {
            match tokio::time::timeout_at(deadline, self.read_frame()).await {
                Err(_) => return Ok(()),
                Ok(frame) => {
                    let frame = frame?;
                    if frame.id == ClientboundKeepAlive::ID {
                        let ka = ClientboundKeepAlive::decode(&mut frame.body.clone())?;
                        self.write_packet(&ServerboundKeepAlive { id: ka.id })
                            .await?;
                    } else if frame.id == SynchronizePlayerPosition::ID {
                        let sync = SynchronizePlayerPosition::decode(&mut frame.body.clone())?;
                        self.write_packet(&ConfirmTeleportation::accepting(&sync))
                            .await?;
                    }
                    seen.push(frame);
                }
            }
        }
    }
}

async fn drive_to_play(cli: &Cli) -> Result<(Probe, i32, SynchronizePlayerPosition)> {
    let stream = TcpStream::connect(&cli.addr)
        .await
        .with_context(|| format!("connect to {}", cli.addr))?;
    stream.set_nodelay(true)?;
    let mut probe = Probe::new(stream);
    let port = cli
        .addr
        .rsplit_once(':')
        .map(|(_, p)| p.parse().unwrap_or(25565))
        .unwrap_or(25565);
    probe
        .write_packet(&Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: port,
            next_state: NextState::Login,
        })
        .await?;
    probe
        .write_packet(&LoginStart {
            name: cli.name.clone(),
            player_uuid: Uuid::nil(),
        })
        .await?;
    // Login: SetCompression may arrive first.
    loop {
        let frame = probe.read_frame().await?;
        if frame.id == 0x03
            && frame.body.len() <= 5
            && let Ok(threshold) = mc_protocol::codec::ReadMc::read_varint(&mut frame.body.clone())
        {
            probe.compression = if threshold < 0 {
                Compression::Disabled
            } else {
                Compression::Threshold(threshold as usize)
            };
            continue;
        }
        if frame.id == LoginSuccess::ID {
            let mut body = frame.body.clone();
            let _ = LoginSuccess::decode(&mut body)?;
            probe.write_packet(&LoginAcknowledged).await?;
            break;
        }
        if frame.id == 0x00 {
            bail!("login rejected: {}", String::from_utf8_lossy(&frame.body));
        }
    }
    // Configuration: echo known packs, ack finish.
    let mut sent_packs = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let frame = tokio::time::timeout_at(deadline, probe.read_frame())
            .await
            .context("timed out in configuration")??;
        if !sent_packs
            && let Ok(known) = ClientboundKnownPacks::decode(&mut frame.body.clone())
            && !known.packs.is_empty()
        {
            probe
                .write_packet(&ServerboundKnownPacks {
                    packs: known.packs.clone(),
                })
                .await?;
            sent_packs = true;
            continue;
        }
        if frame.body.is_empty() && sent_packs {
            probe.write_packet(&AcknowledgeFinishConfiguration).await?;
            break;
        }
    }
    // Play entry: LoginPlay then drain until first SynchronizePlayerPosition.
    let mut entity_id = 0;
    let sync = loop {
        let frame = tokio::time::timeout(Duration::from_secs(30), probe.read_frame())
            .await
            .context("timed out entering play")??;
        if frame.id == ClientboundKeepAlive::ID {
            let ka = ClientboundKeepAlive::decode(&mut frame.body.clone())?;
            probe
                .write_packet(&ServerboundKeepAlive { id: ka.id })
                .await?;
            continue;
        }
        if frame.id == LoginPlay::ID {
            let login = LoginPlay::decode(&mut frame.body.clone())?;
            entity_id = login.entity_id;
            continue;
        }
        if frame.id == SynchronizePlayerPosition::ID {
            let sync = SynchronizePlayerPosition::decode(&mut frame.body.clone())?;
            probe
                .write_packet(&ConfirmTeleportation::accepting(&sync))
                .await?;
            break sync;
        }
    };
    probe.write_packet(&ServerboundPlayerLoaded).await?;
    // Orchestrator handshake (kills ALL setup races): pump >= 20
    // ClientTickEnd barriers so chunks around the player are loaded, print
    // one JSON line {"event":"ready"} on stdout, then BLOCK reading stdin
    // until a "go" line arrives. The orchestrator sends console setup
    // (give/tp/summon) after "ready" and only then writes "go".
    for _ in 0..20 {
        probe.write_packet(&ServerboundClientTickEnd).await?;
        probe
            .drain(Duration::from_millis(200), &mut Vec::new())
            .await?;
    }
    println!("{}", serde_json::to_string(&json!({"event": "ready"}))?);
    {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                bail!("stdin closed before go");
            }
            if line.trim() == "go" {
                break;
            }
        }
    }
    Ok((probe, entity_id, sync))
}

/// Tracked victim health from ClientboundSetEntityData (living health index 9).
fn health_of(frame: &RawFrame, victim: i32) -> Option<f32> {
    if frame.id != ClientboundSetEntityData::ID {
        return None;
    }
    let data = ClientboundSetEntityData::decode(&mut frame.body.clone()).ok()?;
    if data.entity_id != victim {
        return None;
    }
    for v in &data.values {
        if let EntityDataValue::Float { index: 9, value } = v {
            return Some(*value);
        }
    }
    None
}

/// Find a zombie AddEntity (any entity that is not our player).
fn add_entity_id(frame: &RawFrame, self_id: i32) -> Option<(i32, i32)> {
    if frame.id != AddEntity::ID {
        return None;
    }
    let e = AddEntity::decode(&mut frame.body.clone()).ok()?;
    if e.entity_id == self_id {
        return None;
    }
    Some((e.entity_id, e.entity_type_id))
}

/// Spawn position of a non-self AddEntity (for victim disambiguation).
fn add_entity_pos(frame: &RawFrame) -> Option<[f64; 3]> {
    if frame.id != AddEntity::ID {
        return None;
    }
    let e = AddEntity::decode(&mut frame.body.clone()).ok()?;
    Some([e.x, e.y, e.z])
}

/// One-line frame census for wait-timeout diagnostics (no payload decode).
fn census(seen: &[RawFrame]) -> String {
    let mut counts: std::collections::BTreeMap<i32, usize> = Default::default();
    for f in seen {
        *counts.entry(f.id).or_default() += 1;
    }
    format!("frames={} ids={counts:?}", seen.len())
}
/// Wait for N server ticks, event-driven: ClientboundSetTime advances every
/// 20 ticks and KeepAlive proves liveness; count SetTime frames as the tick
/// clock so waits track the server instead of wall-clock sleeping.
async fn wait_server_ticks(probe: &mut Probe, seen: &mut Vec<RawFrame>, ticks: u64) -> Result<()> {
    // SetTime id 0x?? — resolve via the packet type, not memory.
    let target_game_time: Option<i64> = seen
        .iter()
        .rev()
        .filter(|f| f.id == mc_protocol::packets::play::ClientboundSetTime::ID)
        .find_map(|f| {
            mc_protocol::packets::play::ClientboundSetTime::decode(&mut f.body.clone())
                .ok()
                .map(|t| t.game_time)
        });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10 + ticks * 2);
    let mut elapsed_ticks = 0_u64;
    let mut last_time = target_game_time;
    // Pump ClientTickEnd barriers; each server tick advances game time by 1.
    while elapsed_ticks < ticks {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        probe.write_packet(&ServerboundClientTickEnd).await?;
        // Wait for any frame (or 200ms quiet = one tick passed with no news).
        match tokio::time::timeout(Duration::from_millis(200), probe.read_frame()).await {
            Err(_) => {
                elapsed_ticks += 1;
            }
            Ok(frame) => {
                let frame = frame?;
                if frame.id == ClientboundKeepAlive::ID {
                    let ka = ClientboundKeepAlive::decode(&mut frame.body.clone())?;
                    probe
                        .write_packet(&ServerboundKeepAlive { id: ka.id })
                        .await?;
                } else if frame.id == SynchronizePlayerPosition::ID {
                    let sync = SynchronizePlayerPosition::decode(&mut frame.body.clone())?;
                    probe
                        .write_packet(&ConfirmTeleportation::accepting(&sync))
                        .await?;
                }
                if frame.id == mc_protocol::packets::play::ClientboundSetTime::ID
                    && let Ok(t) = mc_protocol::packets::play::ClientboundSetTime::decode(
                        &mut frame.body.clone(),
                    )
                {
                    if let Some(last) = last_time {
                        elapsed_ticks += (t.game_time - last).max(0) as u64;
                    }
                    last_time = Some(t.game_time);
                }
                seen.push(frame);
            }
        }
    }
    Ok(())
}

async fn scenario_attribute_swap(
    probe: &mut Probe,
    self_id: i32,
    seen: &mut Vec<RawFrame>,
) -> Result<serde_json::Value> {
    // Wait for the victim to appear (orchestrator summoned it already; watch
    // for a fresh AddEntity) and for hotbar content showing the issued items.
    // Vanilla sends a full ContainerSetContent image; Solaris sends per-slot
    // MenuSlot deltas (ClientboundContainerSetSlot) — accumulate both into
    // one 46-slot image so either server form is observed. Event-driven: no
    // sleeps, just frame waits.
    let mut victim: Option<i32> = None;
    let mut entities_seen: Vec<(i32, i32, [f64; 3])> = Vec::new();
    let mut image: Vec<String> = vec!["empty".to_string(); 46];
    let mut slots_seen: usize;
    // Long deadline: the orchestrator retries setup until the probe finishes,
    // so the wait must outlast several setup rounds. WAIT FOR THE VICTIM
    // FIRST (zombies only), THEN for inventory — never one joint condition:
    // the joint form latches a player-echo entity and starves on slots.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    while tokio::time::Instant::now() < deadline && victim.is_none() {
        let frame = tokio::time::timeout_at(deadline, probe.read_frame())
            .await
            .map_err(|_| anyhow::anyhow!("waiting for victim: timeout — {}", census(seen)))?
            .context("waiting for victim (connection closed)")?;
        if frame.id == ClientboundKeepAlive::ID {
            let ka = ClientboundKeepAlive::decode(&mut frame.body.clone())?;
            probe
                .write_packet(&ServerboundKeepAlive { id: ka.id })
                .await?;
            seen.push(frame);
            continue;
        }
        if frame.id == SynchronizePlayerPosition::ID {
            let sync = SynchronizePlayerPosition::decode(&mut frame.body.clone())?;
            probe
                .write_packet(&ConfirmTeleportation::accepting(&sync))
                .await?;
            seen.push(frame);
            continue;
        }
        if let Some((id, ty)) = add_entity_id(&frame, self_id) {
            // Track every non-self entity; drop item drops (71) only.
            // Registry ids verified via data/vanilla/reports/registries.json
            // (zombie=150, player=155, item=71) — never memory. The login
            // sequence emits leftover player/item entities; the candidate
            // retry loop (named-first, then arena distance, then first
            // health that moves) sorts the victim from the clutter.
            if ty == 71 {
                seen.push(frame);
                continue;
            }
            if !entities_seen.iter().any(|(seen_id, _, _)| *seen_id == id) {
                // Spawn position: the victim is summoned at the KNOWN arena
                // spot, so later we prefer the entity closest to it over
                // stale entities from earlier setup rounds.
                let pos = add_entity_pos(&frame).unwrap_or([1e9, 1e9, 1e9]);
                entities_seen.push((id, ty, pos));
            }
            // Latch the first ZOMBIE (registry id 150) spawned AT the arena spot:
            // Solaris runs on real worldgen, so unrelated zombies stream in
            // before the summoned victim reaches the teleported player. The
            // retry loop still re-syncs to whichever candidate's health moves.
            let at_arena = add_entity_pos(&frame).is_some_and(|p| {
                (p[0] - 0.5).abs() < 2.0 && (p[1] - 64.0).abs() < 2.0 && (p[2] - 2.5).abs() < 2.0
            });
            if victim.is_none() && ty == 150 && at_arena {
                victim = Some(id);
            }
        }
        seen.push(frame);
    }
    // Phase 2: inventory image (ContainerSetContent full image on vanilla,
    // per-slot MenuSlot deltas on Solaris) — same keepalive/teleport
    // handling, separate deadline so a latched victim never starves it.
    // Frames consumed while waiting for the victim (Solaris streams the gives
    // before the teleported player sees the arena zombie) may already carry
    // the hotbar: replay them into the image before waiting for more.
    for frame in seen.iter() {
        if frame.id == ClientboundContainerSetContent::ID
            && let Ok(c) = ClientboundContainerSetContent::decode(&mut frame.body.clone())
            && c.container_id == 0
        {
            for (i, s) in c.items.iter().enumerate().take(46) {
                if !s.is_empty() {
                    image[i] = format!("{}x{}", s.item_id, s.count);
                }
            }
        }
        if frame.id == ClientboundContainerSetSlot::ID
            && let Ok(s) = ClientboundContainerSetSlot::decode(&mut frame.body.clone())
            && s.container_id == 0
            && s.slot >= 0
        {
            let idx = s.slot as usize;
            if idx < 46 && !s.item_stack.is_empty() {
                image[idx] = format!("{}x{}", s.item_stack.item_id, s.item_stack.count);
            }
        }
    }
    slots_seen = image.iter().filter(|s| *s != "empty").count();
    while tokio::time::Instant::now() < deadline && slots_seen < 2 {
        let frame = tokio::time::timeout_at(deadline, probe.read_frame())
            .await
            .context("waiting for inventory (ContainerSetContent/SetSlot)")??;
        if frame.id == ClientboundKeepAlive::ID {
            let ka = ClientboundKeepAlive::decode(&mut frame.body.clone())?;
            probe
                .write_packet(&ServerboundKeepAlive { id: ka.id })
                .await?;
            seen.push(frame);
            continue;
        }
        if frame.id == SynchronizePlayerPosition::ID {
            let sync = SynchronizePlayerPosition::decode(&mut frame.body.clone())?;
            probe
                .write_packet(&ConfirmTeleportation::accepting(&sync))
                .await?;
            seen.push(frame);
            continue;
        }
        if frame.id == ClientboundContainerSetContent::ID
            && let Ok(c) = ClientboundContainerSetContent::decode(&mut frame.body.clone())
            && c.container_id == 0
        {
            for (i, s) in c.items.iter().enumerate().take(46) {
                if !s.is_empty() {
                    image[i] = format!("{}x{}", s.item_id, s.count);
                }
            }
            slots_seen = image.iter().filter(|s| *s != "empty").count();
        }
        if frame.id == ClientboundContainerSetSlot::ID
            && let Ok(s) = ClientboundContainerSetSlot::decode(&mut frame.body.clone())
            && s.container_id == 0
            && s.slot >= 0
        {
            let idx = s.slot as usize;
            if idx < 46 && !s.item_stack.is_empty() {
                image[idx] = format!("{}x{}", s.item_stack.item_id, s.item_stack.count);
                slots_seen = image.iter().filter(|s| *s != "empty").count();
            }
        }
        seen.push(frame);
    }
    let victim = victim.context("no victim entity observed (summon failed?)")?;
    // The victim's initial health may not have been sent yet (we joined after
    // the summon). Poke one ClientTickEnd round so its SetEntityData arrives.
    for _ in 0..3 {
        probe.write_packet(&ServerboundClientTickEnd).await?;
        probe.drain(Duration::from_millis(200), seen).await?;
    }
    let health_before = seen.iter().filter_map(|f| health_of(f, victim)).last();
    let hotbar: Vec<String> = image[36..45].to_vec();
    let last_health_for = |seen: &[RawFrame], id: i32| -> Option<f32> {
        seen.iter().filter_map(|f| health_of(f, id)).last()
    };
    let last_health = |seen: &[RawFrame]| -> Option<f32> { last_health_for(seen, victim) };
    // Select slot 0 (sword) explicitly, then idle >= 22 server ticks so
    // attack strength is full on both servers.
    probe
        .write_packet(&ServerboundSetCarriedItem { slot: 0 })
        .await?;
    wait_server_ticks(probe, seen, 25).await?;
    // Case A: attack with sword, no swap (control). The latched victim may be
    // stale (re-summoned by a later setup round, or an unrelated entity), so
    // retry across ALL entities seen so far: attack each candidate once and
    // keep the first one whose health actually moves. This also re-syncs the
    // victim id for cases B/C/D.
    let mut victim = victim;
    let mut dmg_a: Option<f32> = None;
    // Solaris spawns in a NORMAL world: summon coordinates land in unloaded
    // terrain, so the victim may be FAR from the arena. Before attacking,
    // walk to the victim's spawn position — vanilla reach (~3 blocks) then
    // applies on BOTH servers. Vanilla arena victims sit at ARENA already,
    // so the move is a no-op there (same spot).
    const ARENA: [f64; 3] = [0.5, 64.0, 2.5];
    // Order candidates: the NAMED victim ("Victim" custom name, spawned at
    // the arena spot) first, then by distance to the arena spot (0.5,64,2.5).
    // Custom-name matching needs EntityData index 2 (optional chat component
    // containing "Victim") — scored from already-seen SetEntityData frames.
    let named: Vec<i32> = seen
        .iter()
        .filter(|f| f.id == ClientboundSetEntityData::ID)
        .filter_map(|f| {
            let d = ClientboundSetEntityData::decode(&mut f.body.clone()).ok()?;
            let is_victim = d.values.iter().any(|v| format!("{v:?}").contains("Victim"));
            is_victim.then_some(d.entity_id)
        })
        .collect();
    let mut candidates: Vec<i32> = {
        let mut with_key: Vec<((u8, f64), i32)> = entities_seen
            .iter()
            .map(|(id, _, pos)| {
                let d = (pos[0] - ARENA[0]).powi(2)
                    + (pos[1] - ARENA[1]).powi(2)
                    + (pos[2] - ARENA[2]).powi(2);
                let rank: u8 = if named.contains(id) { 0 } else { 1 };
                ((rank, d), *id)
            })
            .collect();
        with_key.sort_by(|a, b| a.0.0.cmp(&b.0.0).then(a.0.1.total_cmp(&b.0.1)));
        with_key.into_iter().map(|(_, id)| id).collect()
    };
    if !candidates.contains(&victim) {
        candidates.insert(0, victim);
    }
    for candidate in candidates {
        // Walk to THIS candidate's spawn (minus 1.7 blocks Z) before each
        // attack: on Solaris the summon lands in unloaded terrain far from
        // the player, and attacks beyond reach never land. Vanilla arena
        // victims sit at ARENA so this is a no-op there.
        if let Some((_, _, pos)) = entities_seen.iter().find(|(id, _, _)| *id == candidate) {
            probe
                .write_packet(&ServerboundMovePlayerPosRot {
                    x: pos[0],
                    y: pos[1].max(64.0),
                    z: pos[2] - 1.7,
                    yaw: 0.0,
                    pitch: 0.0,
                    flags: MovePlayerFlags::new(true, false),
                })
                .await?;
            wait_server_ticks(probe, seen, 10).await?;
        }
        let hb = last_health_for(seen, candidate);
        probe
            .write_packet(&ServerboundAttack {
                entity_id: candidate,
            })
            .await?;
        probe.write_packet(&ServerboundPunch).await?;
        wait_server_ticks(probe, seen, 16).await?;
        // Learn about zombies that appeared mid-run (later setup rounds).
        let fresh: Vec<(i32, i32, [f64; 3])> = seen
            .iter()
            .filter_map(|frame| {
                let (id, ty) = add_entity_id(frame, self_id)?;
                (ty != 71).then_some(())?;
                let pos = add_entity_pos(frame).unwrap_or([1e9, 1e9, 1e9]);
                Some((id, ty, pos))
            })
            .collect();
        for (id, ty, pos) in fresh {
            if !entities_seen.iter().any(|(seen_id, _, _)| *seen_id == id) {
                entities_seen.push((id, ty, pos));
            }
        }
        let ha = last_health_for(seen, candidate);
        if let (Some(b), Some(a)) = (hb, ha)
            && (b - a).abs() > 0.001
        {
            victim = candidate;
            dmg_a = Some(b - a);
            break;
        }
        wait_server_ticks(probe, seen, 70).await?;
    }
    // Full recharge + hurt-cooldown clear between EVERY attack: attack
    // strength needs ~22 ticks (1.1s) after the previous swing, and the
    // hurt cooldown (0.5s = 10 ticks) zeroes damage if the previous hit was
    // too recent. Vanilla truth run showed reduced damage when re-hits came
    // too fast; recharge fully so EVERY case measures full-strength damage.
    wait_server_ticks(probe, seen, 60).await?;
    // Case B: swap to stick, wait 2 ticks, attack (attributes updated).
    probe
        .write_packet(&ServerboundSetCarriedItem { slot: 1 })
        .await?;
    wait_server_ticks(probe, seen, 2).await?;
    let hb = last_health(seen);
    probe
        .write_packet(&ServerboundAttack { entity_id: victim })
        .await?;
    probe.write_packet(&ServerboundPunch).await?;
    wait_server_ticks(probe, seen, 16).await?;
    let ha = last_health(seen);
    let dmg_b = match (hb, ha) {
        (Some(b), Some(a)) => Some(b - a),
        _ => None,
    };
    wait_server_ticks(probe, seen, 70).await?;
    // Case C: same-tick swap sword->stick + attack in ONE TCP write.
    probe
        .write_packet(&ServerboundSetCarriedItem { slot: 0 })
        .await?;
    wait_server_ticks(probe, seen, 25).await?;
    let hb = last_health(seen);
    let mut b1 = BytesMut::new();
    ServerboundSetCarriedItem { slot: 1 }.encode(&mut b1)?;
    let f1 = encode_frame(ServerboundSetCarriedItem::ID, &b1, probe.compression)?;
    let mut b2 = BytesMut::new();
    ServerboundAttack { entity_id: victim }.encode(&mut b2)?;
    let f2 = encode_frame(ServerboundAttack::ID, &b2, probe.compression)?;
    let mut both = Vec::with_capacity(f1.len() + f2.len());
    both.extend_from_slice(&f1);
    both.extend_from_slice(&f2);
    probe.stream.write_all(&both).await?;
    probe.write_packet(&ServerboundPunch).await?;
    wait_server_ticks(probe, seen, 16).await?;
    let ha = last_health(seen);
    let dmg_c = match (hb, ha) {
        (Some(b), Some(a)) => Some(b - a),
        _ => None,
    };
    wait_server_ticks(probe, seen, 70).await?;
    // Case D: same-tick swap stick->sword + attack in ONE TCP write.
    probe
        .write_packet(&ServerboundSetCarriedItem { slot: 1 })
        .await?;
    wait_server_ticks(probe, seen, 25).await?;
    let hb = last_health(seen);
    let mut b1 = BytesMut::new();
    ServerboundSetCarriedItem { slot: 0 }.encode(&mut b1)?;
    let f1 = encode_frame(ServerboundSetCarriedItem::ID, &b1, probe.compression)?;
    let mut b2 = BytesMut::new();
    ServerboundAttack { entity_id: victim }.encode(&mut b2)?;
    let f2 = encode_frame(ServerboundAttack::ID, &b2, probe.compression)?;
    let mut both = Vec::with_capacity(f1.len() + f2.len());
    both.extend_from_slice(&f1);
    both.extend_from_slice(&f2);
    probe.stream.write_all(&both).await?;
    probe.write_packet(&ServerboundPunch).await?;
    wait_server_ticks(probe, seen, 16).await?;
    let ha = last_health(seen);
    let dmg_d = match (hb, ha) {
        (Some(b), Some(a)) => Some(b - a),
        _ => None,
    };
    let corrections = seen
        .iter()
        .filter(|f| f.id == SynchronizePlayerPosition::ID)
        .count();
    // Diagnostics: every observed victim-health value in order, plus damage
    // events on the victim — distinguishes "attack never landed" from
    // "health frames never arrived".
    let health_timeline: Vec<f32> = seen.iter().filter_map(|f| health_of(f, victim)).collect();
    let damage_events: usize = seen
        .iter()
        .filter(|f| {
            f.id == ClientboundDamageEvent::ID
                && ClientboundDamageEvent::decode(&mut f.body.clone())
                    .is_ok_and(|d| d.entity_id == victim)
        })
        .count();
    Ok(json!({
        "victim_id": victim,
        "entities_seen": entities_seen,
        "health_before": health_before,
        "hotbar": hotbar,
        "damage_no_swap_sword": dmg_a,
        "damage_swap_wait_stick": dmg_b,
        "damage_same_tick_sword_then_stick": dmg_c,
        "damage_same_tick_stick_then_sword": dmg_d,
        "corrections": corrections,
        "health_timeline": health_timeline,
        "damage_events": damage_events,
        "complete": dmg_a.is_some() && dmg_b.is_some() && dmg_c.is_some() && dmg_d.is_some(),
    }))
}

/// Per-tick ground sprint displacement from vanilla player physics.
///
/// `diagonal`: forward+strafe both 1.0 (un-normalised client input, MC-271065).
/// `scale`: extra multiplier (1.0 = legit, 1.1 = 10% faster control).
fn sprint_step(pos: [f64; 3], yaw_deg: f64, diagonal: bool, scale: f64) -> [f64; 3] {
    // Vanilla: base 0.1 * 1.3 sprint, ground friction 0.6 block (0.91),
    // acceleration impulse 0.1 * (0.6/slip)^3 / 0.91... simplified to the
    // steady-state sprint speed vanilla converges to:
    // straight 0.2805 b/tick, diagonal x sqrt(2) = 0.3967 (MC-271065).
    let straight = 0.2805 * scale;
    let (dx, dz) = if diagonal {
        let s = straight * std::f64::consts::SQRT_2 / 2.0_f64.sqrt() * 2.0_f64.sqrt();
        // unit diagonal direction rotated by yaw: forward(-z) + left(-x) at yaw=0
        let _ = s;
        let v = straight * std::f64::consts::SQRT_2;
        let yaw = yaw_deg.to_radians();
        // forward vector (-sin yaw, -cos yaw... use vanilla moveRelative):
        // forward input 1 => -z at yaw 0; strafe(left) 1 => -x at yaw 0.
        let fx = -yaw.sin();
        let fz = yaw.cos();
        let sx = -fz;
        let sz = fx;
        // un-normalised: magnitude sqrt(2), direction = normalize(f+s)
        let mx = (fx + sx) / std::f64::consts::SQRT_2;
        let mz = (fz + sz) / std::f64::consts::SQRT_2;
        (mx * v, mz * v)
    } else {
        let yaw = yaw_deg.to_radians();
        (-yaw.sin() * straight, yaw.cos() * straight)
    };
    [pos[0] + dx, pos[1], pos[2] + dz]
}

fn last_sync_of(seen: &[RawFrame]) -> Option<SynchronizePlayerPosition> {
    seen.iter()
        .rev()
        .filter(|f| f.id == SynchronizePlayerPosition::ID)
        .find_map(|f| SynchronizePlayerPosition::decode(&mut f.body.clone()).ok())
}

fn sync_count(seen: &[RawFrame]) -> usize {
    seen.iter()
        .filter(|f| f.id == SynchronizePlayerPosition::ID)
        .count()
}

/// Send one move + tick barrier, then wait until the server has consumed it:
/// the next ClientboundSetTime (clock cadence 20 ticks) or KeepAlive proves a
/// server tick pumped. Returns true if a NEW correction arrived on this step.
async fn step_and_sync(
    probe: &mut Probe,
    pos: [f64; 3],
    yaw: f64,
    seen: &mut Vec<RawFrame>,
    known_corrs: &mut usize,
    adopted: &mut [f64; 3],
) -> Result<bool> {
    probe
        .write_packet(&ServerboundMovePlayerPosRot {
            x: pos[0],
            y: pos[1],
            z: pos[2],
            yaw: yaw as f32,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await?;
    probe.write_packet(&ServerboundClientTickEnd).await?;
    // Event-driven wait: one 300ms frame window per step. A correction
    // frame ends the step immediately (adopt it); anything else is just
    // recorded. Silence means the tick consumed the move without complaint.
    // (300ms comfortably covers a 50ms server tick plus network jitter.)
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    let mut corrected = false;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        match tokio::time::timeout(deadline - now, probe.read_frame()).await {
            Err(_) => break,
            Ok(frame) => {
                let frame = frame?;
                if frame.id == ClientboundKeepAlive::ID {
                    let ka = ClientboundKeepAlive::decode(&mut frame.body.clone())?;
                    probe
                        .write_packet(&ServerboundKeepAlive { id: ka.id })
                        .await?;
                } else if frame.id == SynchronizePlayerPosition::ID {
                    let sync = SynchronizePlayerPosition::decode(&mut frame.body.clone())?;
                    probe
                        .write_packet(&ConfirmTeleportation::accepting(&sync))
                        .await?;
                }
                seen.push(frame);
                let n = sync_count(seen);
                if n > *known_corrs {
                    *known_corrs = n;
                    if let Some(s) = last_sync_of(seen) {
                        *adopted = [s.x, s.y, s.z];
                    }
                    corrected = true;
                    break;
                }
            }
        }
    }
    Ok(corrected)
}

async fn run_trajectory(
    probe: &mut Probe,
    start: [f64; 3],
    yaw: f64,
    ticks: usize,
    diagonal: bool,
    scale: f64,
    seen: &mut Vec<RawFrame>,
) -> Result<serde_json::Value> {
    // Teleport-free: first walk TO `start` in small legit steps from wherever
    // the server put us (spawn), so vanilla never sees a >10-block jump, then
    // run the measured trajectory from there.
    let base = sync_count(seen);
    let mut known_corrs = base;
    probe
        .write_packet(&ServerboundPlayerInput {
            input: PlayerInput {
                forward: true,
                left: diagonal,
                right: false,
                backward: false,
                jump: false,
                shift: false,
                sprint: true,
            },
        })
        .await?;
    let mut pos: [f64; 3] = last_sync_of(seen).map(|s| [s.x, s.y, s.z]).unwrap_or(start);
    let mut adopted = pos;
    // Anchor: one no-op move at the current position so vanilla's move
    // checker has a sane baseline (avoids a first-step teleport verdict).
    step_and_sync(probe, pos, yaw, seen, &mut known_corrs, &mut adopted).await?;
    pos = adopted;
    let mut guard = 0;
    while ((pos[0] - start[0]).powi(2) + (pos[2] - start[2]).powi(2)).sqrt() > 0.3 && guard < 400 {
        guard += 1;
        let dx = start[0] - pos[0];
        let dz = start[2] - pos[2];
        let len = (dx * dx + dz * dz).sqrt().max(1e-9);
        let step = 0.28_f64.min(len);
        pos = [pos[0] + dx / len * step, start[1], pos[2] + dz / len * step];
        if step_and_sync(probe, pos, yaw, seen, &mut known_corrs, &mut adopted).await? {
            pos = adopted;
            break;
        }
        pos = adopted;
    }
    let walk_corrections = sync_count(seen) - base;
    known_corrs = sync_count(seen);
    pos[1] = start[1];
    let traj_start = pos;
    let mut traj_corrections = 0_usize;
    for _ in 0..ticks {
        pos = sprint_step(pos, yaw, diagonal, scale);
        if step_and_sync(probe, pos, yaw, seen, &mut known_corrs, &mut adopted).await? {
            traj_corrections += 1;
            pos = adopted;
        } else {
            pos = adopted;
        }
    }
    // Settle: one barrier + one 1s drain so late corrections arrive.
    probe.write_packet(&ServerboundClientTickEnd).await?;
    probe.drain(Duration::from_secs(1), seen).await?;
    let total_new = sync_count(seen) - base - walk_corrections;
    let last_correction = last_sync_of(seen).map(|s| [s.x, s.y, s.z]);
    Ok(json!({
        "traj_start": traj_start,
        "sent": pos,
        "corrections": total_new.max(traj_corrections),
        "per_step_corrections": traj_corrections,
        "walk_corrections": walk_corrections,
        "last_correction": last_correction,
    }))
}

async fn scenario_diagonal_speed(
    probe: &mut Probe,
    _self_id: i32,
    spawn: SynchronizePlayerPosition,
    seen: &mut Vec<RawFrame>,
) -> Result<serde_json::Value> {
    // Drain spawn burst.
    probe.drain(Duration::from_secs(2), seen).await?;
    let start = [spawn.x, spawn.y, spawn.z];
    // Straight sprint control. Each trajectory reports its own correction
    // count (walk-to-start corrections excluded).
    let straight = run_trajectory(probe, start, 0.0, 40, false, 1.0, seen).await?;
    // Diagonal sprint (the quirk).
    let diagonal = run_trajectory(probe, start, 0.0, 40, true, 1.0, seen).await?;
    // 10% faster than diagonal (control that vanilla may reject).
    let fast = run_trajectory(probe, start, 0.0, 40, true, 1.1, seen).await?;
    Ok(json!({
        "straight": {"traj_start": straight["traj_start"], "sent": straight["sent"], "corrections": straight["corrections"], "last_correction": straight["last_correction"]},
        "diagonal": {"traj_start": diagonal["traj_start"], "sent": diagonal["sent"], "corrections": diagonal["corrections"], "last_correction": diagonal["last_correction"]},
        "diagonal_fast10": {"traj_start": fast["traj_start"], "sent": fast["sent"], "corrections": fast["corrections"], "last_correction": fast["last_correction"]},
    }))
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let (mut probe, self_id, spawn) = drive_to_play(&cli).await?;
    let mut seen: Vec<RawFrame> = Vec::new();
    let obs = match cli.scenario.as_str() {
        "attribute-swap" => scenario_attribute_swap(&mut probe, self_id, &mut seen).await,
        "diagonal-speed" => scenario_diagonal_speed(&mut probe, self_id, spawn, &mut seen).await,
        _ => Err(anyhow::anyhow!("unknown scenario")),
    };
    // On disconnect/timeout mid-scenario the vanilla side already holds
    // COMPLETE measurements (all four damages); a bail here would discard
    // them. Emit whatever was measured with complete=false instead.
    let obs = match obs {
        Ok(o) => o,
        Err(e) => {
            eprintln!("quirk-probe partial: {e:#} — emitting incomplete observations");
            json!({"partial_error": format!("{e:#}"), "complete": false})
        }
    };
    let health_now: Vec<f32> = Vec::new();
    let _ = health_now;
    // Count packet families for the detail file.
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for f in &seen {
        let name = match f.id {
            x if x == ClientboundSetEntityData::ID => "SetEntityData",
            x if x == ClientboundDamageEvent::ID => "DamageEvent",
            x if x == SynchronizePlayerPosition::ID => "SyncPlayerPos",
            x if x == AddEntity::ID => "AddEntity",
            x if x == RemoveEntities::ID => "RemoveEntities",
            x if x == MoveEntityPosRot::ID => "MoveEntity",
            x if x == SetEntityMotion::ID => "SetEntityMotion",
            x if x == ClientboundContainerSetContent::ID => "ContainerSetContent",
            x if x == ClientboundContainerSetSlot::ID => "SetSlot",
            x if x == ClientboundSetHealth::ID => "SetHealth",
            x if x == ClientboundKeepAlive::ID => "KeepAlive",
            _ => "other",
        };
        *counts.entry(name.to_string()).or_default() += 1;
    }
    let out = json!({
        "scenario": cli.scenario,
        "player": cli.name,
        "self_id": self_id,
        "observations": obs,
        "frame_counts": counts,
    });
    println!("{}", serde_json::to_string(&out)?);
    if let Some(parent) = cli.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&cli.out, serde_json::to_string_pretty(&out)?)?;
    Ok(())
}
