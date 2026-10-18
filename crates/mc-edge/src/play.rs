//! Post-configuration join burst: the Play-entry packet sequence.
//!
//! After the Configuration state finishes, the server sends this fixed
//! prelude before any per-tick fanout packets: `LoginPlay` (0x31) with the
//! dimension ids resolved from [`VanillaData`] registries, difficulty,
//! abilities, held slot, an empty-ish but valid command tree, the
//! teleport-confirm [`SynchronizePlayerPosition`], world border, clock sync,
//! default spawn, the `START_WAITING_FOR_CHUNKS` game event, center chunk +
//! cache radius, then container content, health, and time, in the login-flow
//! order vanilla clients expect. Chunks are not part of the burst: the per-viewer streaming
//! fanout (`plan::plan_viewer`) sends them nearest-first at a paced rate
//! (encoding a whole view square here held the connection task for
//! seconds and starved the client of every other packet).

use mc_core::boundary::{EntityId, TickOutput};
use mc_data::{Identifier, VanillaData};
use mc_protocol::frame::Compression;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundChangeDifficulty, ClientboundContainerSetContent, ClientboundInitializeBorder,
    ClientboundPlayerAbilities, ClientboundRecipeBookSettings, ClientboundSetHealth,
    ClientboundSetHeldSlot, ClientboundSetTime, GameEvent, LoginPlay, SetCenterChunk,
    SetChunkCacheRadius, SetDefaultSpawnPosition, SynchronizePlayerPosition,
};

use crate::connection::Connection;

/// Pack `(x, y, z)` into vanilla's `BlockPos` `i64` representation:
/// `((x & 0x3FFFFFF) << 38) | ((z & 0x3FFFFFF) << 12) | (y & 0xFFF)`.
pub fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    (((x as i64) & 0x3FF_FFFF) << 38) | (((z as i64) & 0x3FF_FFFF) << 12) | ((y as i64) & 0xFFF)
}

/// Resolve a vanilla dimension from the registry announced in Configuration.
fn spawn_dimension(
    data: &VanillaData,
    dimension: mc_core::dimensions::Dimension,
) -> anyhow::Result<(i32, Identifier, Vec<Identifier>)> {
    let ids = crate::dimensions::registry_ids(data)?;
    Ok((
        ids[dimension as usize],
        Identifier::parse(dimension.name())?,
        mc_core::dimensions::Dimension::ALL
            .into_iter()
            .map(|dimension| Identifier::parse(dimension.name()).expect("static dimension"))
            .collect(),
    ))
}

fn abilities_for_mode(game_mode: u8) -> ClientboundPlayerAbilities {
    match game_mode {
        // Creative.
        1 => ClientboundPlayerAbilities {
            invulnerable: true,
            flying: false,
            can_fly: true,
            instabuild: true,
            flying_speed: 0.05,
            walking_speed: 0.1,
        },
        // Spectator (2) and adventure (3 fallthrough is survival-like;
        // adventure cannot fly).
        3 => ClientboundPlayerAbilities {
            invulnerable: true,
            flying: true,
            can_fly: true,
            instabuild: false,
            flying_speed: 0.05,
            walking_speed: 0.1,
        },
        _ => ClientboundPlayerAbilities {
            invulnerable: false,
            flying: false,
            can_fly: false,
            instabuild: game_mode == 1,
            flying_speed: 0.05,
            walking_speed: 0.1,
        },
    }
}

/// Send the Play-entry burst on an already-compressed connection.
///
/// Inputs for [`send_join_burst`].
pub struct JoinBurst<'a> {
    /// Live connection (already compressed).
    pub conn: &'a mut Connection,
    /// Vanilla registries for dimension ids.
    pub data: &'a VanillaData,
    /// Newest tick output (clock for the closing time sync).
    pub out: &'a TickOutput,
    /// Entity id the core assigned this player.
    pub viewer_entity: EntityId,
    pub dimension: mc_core::dimensions::Dimension,
    /// Authoritative spawn.
    pub pos: [f64; 3],
    /// View radius sizing the center-chunk window (`LoginPlay.view_distance`,
    /// `SetChunkCacheRadius`).
    pub view_distance: i32,
    /// Gameplay simulation radius (`LoginPlay.simulation_distance`).
    /// Independent of the view radius after the G5 radius split: shrinking
    /// simulation never shrinks the chunk square the client holds.
    pub simulation_distance: i32,
    /// Selects abilities + the `LoginPlay` mode byte.
    pub game_mode: u8,
    /// Recipe list and recipe book (built once at startup).
    pub recipes: &'a crate::recipes::RecipePackets,
    /// Vanilla operator permission level, zero for normal players.
    pub op_level: u8,
    /// Manifest roots from the actual loaded host.
    pub plugin_commands: &'a [(String, u8)],
}

pub async fn send_join_burst(burst: JoinBurst<'_>) -> anyhow::Result<()> {
    let JoinBurst {
        conn,
        data,
        out,
        viewer_entity,
        dimension,
        pos,
        view_distance,
        simulation_distance,
        game_mode,
        recipes,
        op_level,
        plugin_commands,
    } = burst;
    let (dimension_type_id, dimension_name, dimension_names) = spawn_dimension(data, dimension)?;
    let max_players = i32::try_from(data.entry_count().min(128)).unwrap_or(20);
    let out = out.in_dimension(dimension).unwrap_or(out);

    conn.send_packet(&LoginPlay {
        entity_id: viewer_entity.0,
        is_hardcore: false,
        dimension_names,
        max_players,
        view_distance,
        simulation_distance,
        reduced_debug_info: false,
        enable_respawn_screen: true,
        do_limited_crafting: false,
        dimension_type_id,
        dimension_name: dimension_name.clone(),
        hashed_seed: 0,
        game_mode,
        previous_game_mode: -1,
        is_debug: false,
        is_flat: false,
        death_location: None,
        portal_cooldown: 0,
        sea_level: dimension.sea_level(),
        enforces_secure_chat: false,
    })
    .await?;
    conn.send_packet(&ClientboundChangeDifficulty {
        difficulty: 1,
        locked: false,
    })
    .await?;
    conn.send_packet(&abilities_for_mode(game_mode)).await?;
    conn.send_packet(&ClientboundSetHeldSlot { slot: 0 })
        .await?;
    conn.send_packet(&crate::command_tree::tree(op_level, plugin_commands))
        .await?;

    conn.send_packet(&SynchronizePlayerPosition {
        teleport_id: 1,
        x: pos[0],
        y: pos[1],
        z: pos[2],
        dx: 0.0,
        dy: 0.0,
        dz: 0.0,
        yaw: 0.0,
        pitch: 0.0,
        relative_flags: 0,
    })
    .await?;

    conn.send_packet(&ClientboundInitializeBorder {
        center_x: 0.0,
        center_z: 0.0,
        old_size: 59_999_968.0,
        new_size: 59_999_968.0,
        lerp_time: 0,
        absolute_max_size: 29_999_984,
        warning_blocks: 5,
        warning_time: 15,
    })
    .await?;
    conn.send_packet(&ClientboundSetTime::overworld(
        i64::try_from(out.snapshot.tick).unwrap_or(i64::MAX),
        out.snapshot.day_time,
        out.snapshot.day_time_rate,
    ))
    .await?;
    for (event, value) in [
        (
            if dimension.has_weather() && out.snapshot.raining {
                GameEvent::EVENT_START_RAINING
            } else {
                GameEvent::EVENT_STOP_RAINING
            },
            0.0,
        ),
        (
            GameEvent::EVENT_RAIN_LEVEL_CHANGE,
            if dimension.has_weather() {
                out.snapshot.rain_level
            } else {
                0.0
            },
        ),
        (
            GameEvent::EVENT_THUNDER_LEVEL_CHANGE,
            if dimension.has_weather() {
                out.snapshot.thunder_level
            } else {
                0.0
            },
        ),
    ] {
        conn.send_packet(&GameEvent { event, value }).await?;
    }
    conn.send_packet(&SetDefaultSpawnPosition {
        dimension: dimension_name.clone(),
        position: pack_block_pos(
            pos[0].floor() as i32,
            pos[1].floor() as i32,
            pos[2].floor() as i32,
        ),
        yaw: 0.0,
        pitch: 0.0,
    })
    .await?;
    conn.send_packet(&GameEvent {
        event: GameEvent::EVENT_START_WAITING_FOR_CHUNKS,
        value: 0.0,
    })
    .await?;

    let center_x = (pos[0] / 16.0).floor() as i32;
    let center_z = (pos[2] / 16.0).floor() as i32;
    conn.send_packet(&SetCenterChunk {
        chunk_x: center_x,
        chunk_z: center_z,
    })
    .await?;
    conn.send_packet(&SetChunkCacheRadius {
        radius: view_distance,
    })
    .await?;
    // Announce displays/stonecutter inputs, but start with an empty book.
    // Core RecipesUnlocked replaces it with this player's persisted rewards.
    conn.send_packet(&recipes.update).await?;
    conn.send_packet(&ClientboundRecipeBookSettings::default())
        .await?;
    conn.send_packet(&recipes.book).await?;

    // Window 0 (player inventory) is always 46 slots on the wire.
    conn.send_packet(&ClientboundContainerSetContent {
        container_id: 0,
        state_id: 1,
        items: vec![mc_data::item_stack::ItemStack::EMPTY; 46],
        carried_item: mc_data::item_stack::ItemStack::EMPTY,
    })
    .await?;
    conn.send_packet(&ClientboundSetHealth {
        health: 20.0,
        food: 20,
        saturation: 5.0,
    })
    .await?;
    // Closing time sync: the client renders the world clock from the last
    // one it saw.
    conn.send_packet(&ClientboundSetTime::overworld(
        i64::try_from(out.snapshot.tick).unwrap_or(i64::MAX),
        out.snapshot.day_time,
        out.snapshot.day_time_rate,
    ))
    .await?;
    Ok(())
}

/// Compression threshold applied right after login (vanilla default).
pub const JOIN_COMPRESSION_THRESHOLD: usize = 256;

/// Encode one typed packet body (without framing) — helper for the
/// fanout path, which frames planned packets in bulk.
pub fn encode_packet<P: Packet>(packet: &P) -> Result<Vec<u8>, mc_protocol::CodecError> {
    let mut buf = Vec::new();
    packet.encode(&mut buf)?;
    Ok(buf)
}

/// Current compression of a connection (tests + fanout routing).
pub fn connection_compression(conn: &Connection) -> Compression {
    conn.compression()
}

#[cfg(test)]
#[path = "play_tests.rs"]
mod tests;
