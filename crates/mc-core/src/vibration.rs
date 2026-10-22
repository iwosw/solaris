//! Tick-owner vibration dispatch. One nearest/highest-frequency candidate per listener.
use crate::boundary::{BlockPos, EntityId};
use crate::tick::TickState;
use mc_data::item_nbt::field;
use mc_nbt::{ListTag, Tag};
use std::collections::HashMap;

macro_rules! events {
    ($($variant:ident => $name:literal),* $(,)?) => {
        #[derive(Debug,Clone,Copy,PartialEq,Eq)]
        pub(crate) enum GameEvent { $($variant,)* Resonate(u8) }
        impl GameEvent {
            pub(crate) fn name(self) -> &'static str {
                const RESONATE: [&str;15] = ["resonate_1","resonate_2","resonate_3","resonate_4","resonate_5","resonate_6","resonate_7","resonate_8","resonate_9","resonate_10","resonate_11","resonate_12","resonate_13","resonate_14","resonate_15"];
                match self { $(Self::$variant => $name,)* Self::Resonate(n) => RESONATE.get(n.wrapping_sub(1) as usize).copied().unwrap_or("") }
            }
            fn parse(name:&str)->Option<Self> {
                match name.strip_prefix("minecraft:").unwrap_or(name) {
                    $($name => Some(Self::$variant),)*
                    n => n.strip_prefix("resonate_").and_then(|n|n.parse::<u8>().ok()).filter(|n|(1..=15).contains(n)).map(Self::Resonate),
                }
            }
        }
    }
}
events! {
    Step=>"step",Swim=>"swim",Flap=>"flap",ProjectileLand=>"projectile_land",HitGround=>"hit_ground",Splash=>"splash",
    ItemInteractFinish=>"item_interact_finish",ProjectileShoot=>"projectile_shoot",InstrumentPlay=>"instrument_play",
    EntityAction=>"entity_action",ElytraGlide=>"elytra_glide",Unequip=>"unequip",EntityDismount=>"entity_dismount",Equip=>"equip",
    EntityInteract=>"entity_interact",Shear=>"shear",EntityMount=>"entity_mount",EntityDamage=>"entity_damage",Drink=>"drink",Eat=>"eat",
    ContainerClose=>"container_close",BlockClose=>"block_close",BlockDeactivate=>"block_deactivate",BlockDetach=>"block_detach",
    ContainerOpen=>"container_open",BlockOpen=>"block_open",BlockActivate=>"block_activate",BlockAttach=>"block_attach",PrimeFuse=>"prime_fuse",NoteBlockPlay=>"note_block_play",
    BlockChange=>"block_change",BlockDestroy=>"block_destroy",FluidPickup=>"fluid_pickup",BlockPlace=>"block_place",FluidPlace=>"fluid_place",
    EntityPlace=>"entity_place",LightningStrike=>"lightning_strike",Teleport=>"teleport",EntityDie=>"entity_die",Explode=>"explode",
    Shriek=>"shriek",TendrilsClicking=>"tendrils_clicking"
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Vibration {
    pub(crate) event: GameEvent,
    pub(crate) pos: [f64; 3],
    pub(crate) source: Option<EntityId>,
    pub(crate) frequency: u8,
    pub(crate) distance: f64,
    source_uuid: Option<uuid::Uuid>,
    projectile_owner_uuid: Option<uuid::Uuid>,
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Pending {
    pub(crate) vibration: Vibration,
    pub(crate) arrival: u64,
    selected: u64,
}
#[derive(Default)]
pub(crate) struct Bus {
    listeners: HashMap<BlockPos, Listener>,
    dirty: Vec<BlockPos>,
}
struct Listener {
    pos: [f64; 3],
    radius: u8,
    pending: Option<Pending>,
}
pub(crate) fn listen(state: &mut TickState, key: BlockPos, pos: [f64; 3], radius: u8) {
    let entry = state.vibrations.listeners.entry(key).or_insert(Listener {
        pos,
        radius,
        pending: None,
    });
    entry.pos = pos;
    entry.radius = radius;
}
pub(crate) fn unlisten(state: &mut TickState, key: BlockPos) {
    state.vibrations.listeners.remove(&key);
}

pub(crate) fn evict_unloaded(state: &mut TickState) {
    state.vibrations.listeners.retain(|pos, _| {
        state
            .world
            .has_chunk(pos.x.div_euclid(16), pos.z.div_euclid(16))
    });
    state.vibrations.dirty.retain(|pos| {
        state
            .world
            .has_chunk(pos.x.div_euclid(16), pos.z.div_euclid(16))
    });
}

#[cfg(test)]
pub(crate) fn listener_count(state: &TickState) -> usize {
    state.vibrations.listeners.len()
}
pub(crate) fn take(state: &mut TickState, key: BlockPos) -> Option<Vibration> {
    let listener = state.vibrations.listeners.get_mut(&key)?;
    if listener
        .pending
        .as_ref()
        .is_none_or(|p| state.tick < p.arrival || state.tick <= p.selected)
    {
        return None;
    }
    let mut v = listener.pending.take()?.vibration;
    if v.source.is_none() {
        v.source = v
            .projectile_owner_uuid
            .and_then(|uuid| state.entities.slot_of_uuid(uuid))
            .or_else(|| {
                v.source_uuid
                    .and_then(|uuid| state.entities.slot_of_uuid(uuid))
            })
            .map(|s| state.entities.body(s).id);
    }
    Some(v)
}
pub(crate) fn pending(state: &TickState, key: BlockPos) -> Option<&Pending> {
    state.vibrations.listeners.get(&key)?.pending.as_ref()
}
pub(crate) fn restore(state: &mut TickState, key: BlockPos, pending: Option<Pending>) {
    if let Some(l) = state.vibrations.listeners.get_mut(&key) {
        l.pending = pending;
    }
}
pub(crate) fn centre(pos: BlockPos) -> [f64; 3] {
    [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5]
}
pub(crate) fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt()
}
fn tagged(state: &TickState, pos: [i32; 3], raw_ids: &[i32]) -> bool {
    state
        .world
        .block_at(pos[0], pos[1], pos[2])
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|b| raw_ids.contains(&b.block.raw_id))
}
/// Wool is a full-cell acoustic barrier; other solid blocks transmit vibrations.
pub(crate) fn occluded(state: &TickState, from: [f64; 3], to: [f64; 3]) -> bool {
    // A ray can inspect 1024 cells. Resolve the tag once for this read-only
    // traversal, without a cross-event cache that could outlive registry edits.
    let barriers = state
        .registry
        .tags
        .raw_ids("minecraft:block", "minecraft:occludes_vibration_signals")
        .unwrap_or(&[]);
    let a = from.map(|v| v.floor() as i32);
    let b = to.map(|v| v.floor() as i32);
    let mut cell = a;
    let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let step = delta.map(|v| {
        if v > 0.0 {
            1
        } else if v < 0.0 {
            -1
        } else {
            0
        }
    });
    let mut next = [f64::INFINITY; 3];
    let mut stride = [f64::INFINITY; 3];
    for i in 0..3 {
        if step[i] != 0 {
            stride[i] = 1.0 / delta[i].abs();
            next[i] = ((cell[i] + if step[i] > 0 { 1 } else { 0 }) as f64 - from[i]) / delta[i];
        }
    }
    for _ in 0..1024 {
        if tagged(state, cell, barriers) {
            return true;
        }
        if cell == b {
            return false;
        }
        let axis = if next[0] <= next[1] && next[0] <= next[2] {
            0
        } else if next[1] <= next[2] {
            1
        } else {
            2
        };
        cell[axis] += step[axis];
        next[axis] += stride[axis];
    }
    true
}
pub(crate) fn emit(
    state: &mut TickState,
    pos: [f64; 3],
    event: GameEvent,
    source: Option<EntityId>,
) {
    if !pos.iter().all(|v| v.is_finite()) {
        return;
    }
    let source_slot = source.and_then(|id| state.entities.slot_of(id));
    if source.is_some_and(|id| state.players.get(&id).is_some_and(|p| p.spectator())) {
        return;
    }
    if matches!(
        event,
        GameEvent::Step
            | GameEvent::Swim
            | GameEvent::HitGround
            | GameEvent::ProjectileShoot
            | GameEvent::ItemInteractFinish
    ) && source_slot.is_some_and(|s| state.entities.row(s).appearance.shared_flags & 2 != 0)
    {
        // Vanilla awards at listener validation, before occlusion/activation.
        // Sneaking away from every real sensor/warden is not evidence.
        if let Some(player) = source.filter(|id| state.players.contains_key(id))
            && !state.registry.advancements.definitions.is_empty()
            && (state.registry.advancements.trigger_index.is_empty()
                || state
                    .registry
                    .advancements
                    .trigger_index
                    .contains_key("minecraft:avoid_vibration"))
        {
            let sensor = state.vibrations.listeners.iter().any(|(key, listener)| {
                state
                    .world
                    .is_active(key.x.div_euclid(16), key.z.div_euclid(16))
                    && distance(pos, listener.pos) <= f64::from(listener.radius)
                    && listener
                        .pending
                        .as_ref()
                        .is_none_or(|pending| pending.selected == state.tick)
                    && state
                        .world
                        .block_at(key.x, key.y, key.z)
                        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                        .is_some_and(|block| {
                            matches!(
                                block.block.id.path(),
                                "sculk_sensor" | "calibrated_sculk_sensor"
                            )
                        })
            });
            let warden = !sensor
                && state.entities.warden_ids().iter().any(|id| {
                    state.entities.slot_of(*id).is_some_and(|slot| {
                        let at = state.entities.pos(slot);
                        state.entities.body(slot).alive
                            && state.world.is_active(
                                (at[0].floor() as i32).div_euclid(16),
                                (at[2].floor() as i32).div_euclid(16),
                            )
                            && distance(pos, at) <= 16.0
                    })
                });
            if sensor || warden {
                crate::advancements::trigger(
                    state,
                    player,
                    "minecraft:avoid_vibration",
                    serde_json::json!({}),
                );
            }
        }
        return;
    }
    if matches!(event, GameEvent::Step | GameEvent::HitGround)
        && tagged(
            state,
            [
                pos[0].floor() as i32,
                (pos[1] - 0.01).floor() as i32,
                pos[2].floor() as i32,
            ],
            state
                .registry
                .tags
                .raw_ids("minecraft:block", "minecraft:dampens_vibrations")
                .unwrap_or(&[]),
        )
    {
        return;
    }
    let frequency = mc_data::game_events::frequency(event.name());
    if frequency == 0 && event != GameEvent::TendrilsClicking {
        return;
    }
    let mut bus = std::mem::take(&mut state.vibrations);
    for (key, l) in &mut bus.listeners {
        if !state
            .world
            .is_active(key.x.div_euclid(16), key.z.div_euclid(16))
            || pos
                .iter()
                .zip(l.pos)
                .any(|(a, b)| (a - b).abs() > f64::from(l.radius))
        {
            continue;
        }
        if event == GameEvent::TendrilsClicking
            && state
                .world
                .block_at(key.x, key.y, key.z)
                .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                .is_some_and(|b| b.block.id.path() == "sculk_shrieker")
            && source.is_none_or(|id| !state.players.contains_key(&id))
        {
            continue;
        }
        if matches!(event, GameEvent::BlockPlace | GameEvent::BlockDestroy)
            && [key.x, key.y, key.z] == pos.map(|v| v.floor() as i32)
        {
            continue;
        }
        let d = distance(pos, l.pos);
        if d > f64::from(l.radius)
            || occluded(state, pos, l.pos)
            || !crate::sculk::accepts(state, *key, frequency, event)
        {
            continue;
        }
        if let Some(p) = &l.pending
            && (p.selected != state.tick
                || p.vibration.distance < d
                || (p.vibration.distance == d && p.vibration.frequency >= frequency))
        {
            continue;
        }
        l.pending = Some(Pending {
            vibration: Vibration {
                event,
                pos,
                source,
                frequency,
                distance: d,
                source_uuid: source_slot.map(|s| state.entities.row(s).uuid),
                projectile_owner_uuid: None,
            },
            arrival: state
                .tick
                .saturating_add(d.floor() as u64)
                .max(state.tick + 1),
            selected: state.tick,
        });
        if !bus.dirty.contains(key) {
            bus.dirty.push(*key);
        }
    }
    state.vibrations = bus;
    flush(state);
    crate::warden::vibration(state, pos, source);
}
pub(crate) fn tick(state: &mut TickState) {
    crate::sculk::hydrate(state);
    crate::shrieker::hydrate(state);
}

pub(crate) fn flush(state: &mut TickState) {
    let mut dirty = std::mem::take(&mut state.vibrations.dirty);
    dirty.retain(|key| {
        let shrine = state
            .world
            .block_at(key.x, key.y, key.z)
            .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
            .is_some_and(|b| b.block.id.path() == "sculk_shrieker");
        if shrine {
            crate::shrieker::persist_listener(state, *key);
            false
        } else {
            !crate::sculk::persist_listener(state, *key)
        }
    });
    state.vibrations.dirty = dirty;
}

pub(crate) fn intent(state: &mut TickState, intent: &crate::intent::Intent) {
    use crate::intent::IntentKind;
    let (pos, event) = match &intent.kind {
        IntentKind::BreakBlock { pos } => (centre(*pos), GameEvent::BlockDestroy),
        IntentKind::PlaceBlock { pos, state: id } => {
            if state
                .registry
                .blocks
                .by_id(mc_world::BlockStateId(*id))
                .is_some_and(|b| {
                    state.registry.tags.contains_raw_id(
                        "minecraft:block",
                        "minecraft:dampens_vibrations",
                        b.block.raw_id,
                    )
                })
            {
                return;
            }
            (centre(*pos), GameEvent::BlockPlace)
        }
        IntentKind::UseBlock { pos, .. } => (centre(*pos), GameEvent::BlockActivate),
        IntentKind::Bucket { pos, .. } => (centre(*pos), GameEvent::FluidPlace),
        IntentKind::Cultivate { .. } | IntentKind::BlockTool { .. } => {
            let Some((pos, _)) = intent.footprint.block_write.first() else {
                return;
            };
            (centre(*pos), GameEvent::BlockChange)
        }
        IntentKind::Explode { center, .. } | IntentKind::EnvironmentalExplosion { center, .. } => {
            (*center, GameEvent::Explode)
        }
        _ => return,
    };
    if event == GameEvent::BlockDestroy
        && tagged(
            state,
            pos.map(|p| p.floor() as i32),
            state
                .registry
                .tags
                .raw_ids("minecraft:block", "minecraft:dampens_vibrations")
                .unwrap_or(&[]),
        )
    {
        return;
    }
    emit(state, pos, event, Some(intent.actor));
}

pub(crate) fn menu(
    state: &mut TickState,
    connection: crate::boundary::ConnectionId,
    change: &crate::boundary::PlayerChange,
) {
    let event = match change {
        crate::boundary::PlayerChange::MenuOpened { .. } => GameEvent::ContainerOpen,
        crate::boundary::PlayerChange::MenuClosed { .. } => GameEvent::ContainerClose,
        _ => return,
    };
    let Some(entity) = state.connections.entity_of(connection) else {
        return;
    };
    let pos = state
        .open_containers
        .get(&entity)
        .map(|&(x, y, z)| centre(BlockPos { x, y, z }))
        .or_else(|| {
            state
                .entities
                .slot_of(entity)
                .map(|s| state.entities.pos(s))
        });
    if let Some(pos) = pos {
        emit(state, pos, event, Some(entity));
    }
}

/// Vanilla VibrationSystem.Data + VibrationInfo codecs (javap 26.1.2).
pub(crate) fn listener_nbt(pending: Option<&Pending>, tick: u64) -> Tag {
    let mut fields = vec![
        (
            "selector".into(),
            Tag::Compound(vec![("tick".into(), Tag::Long(-1))]),
        ),
        ("event_delay".into(), Tag::Int(0)),
    ];
    if let Some(p) = pending {
        let v = &p.vibration;
        let mut event = vec![
            (
                "game_event".into(),
                Tag::String(format!("minecraft:{}", v.event.name())),
            ),
            ("distance".into(), Tag::Float(v.distance as f32)),
            (
                "pos".into(),
                Tag::List(ListTag {
                    element_type: mc_nbt::tag_type::DOUBLE,
                    elements: v.pos.into_iter().map(Tag::Double).collect(),
                }),
            ),
        ];
        for (name, uuid) in [
            ("source", v.source_uuid),
            ("projectile_owner", v.projectile_owner_uuid),
        ] {
            if let Some(uuid) = uuid {
                event.push((
                    name.into(),
                    Tag::IntArray(
                        uuid.as_bytes()
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|b| i32::from_be_bytes(*b))
                            .collect(),
                    ),
                ));
            }
        }
        fields.push(("event".into(), Tag::Compound(event)));
        fields[1].1 = Tag::Int(p.arrival.saturating_sub(tick).min(i32::MAX as u64) as i32);
    }
    Tag::Compound(fields)
}
pub(crate) fn decode_listener(tag: &Tag, tick: u64) -> Result<Option<Pending>, String> {
    let Tag::Compound(fields) = tag else {
        return Err("listener must be a compound".into());
    };
    let delay = match field(fields, "event_delay") {
        None => 0,
        Some(Tag::Int(n)) if *n >= 0 => *n as u64,
        _ => return Err("listener.event_delay must be a nonnegative int".into()),
    };
    let Some(Tag::Compound(selector)) = field(fields, "selector") else {
        return Err("listener.selector must be a compound".into());
    };
    if !matches!(field(selector, "tick"), Some(Tag::Long(_))) {
        return Err("listener.selector.tick must be a long".into());
    }
    let event = field(fields, "event").or_else(|| field(selector, "event"));
    let Some(event) = event else {
        return Ok(None);
    };
    let Tag::Compound(e) = event else {
        return Err("listener.event must be a compound".into());
    };
    let event = match field(e, "game_event") {
        Some(Tag::String(n)) => {
            GameEvent::parse(n).ok_or("listener.event.game_event is unknown")?
        }
        _ => return Err("listener.event.game_event must be a string".into()),
    };
    let distance = match field(e, "distance") {
        Some(Tag::Float(n)) if n.is_finite() && *n >= 0.0 => f64::from(*n),
        _ => return Err("listener.event.distance must be a nonnegative finite float".into()),
    };
    let pos = match field(e, "pos") {
        Some(Tag::List(l)) if l.elements.len() == 3 => {
            let mut p = [0.0; 3];
            for (i, v) in l.elements.iter().enumerate() {
                let Tag::Double(v) = v else {
                    return Err("listener.event.pos must contain three doubles".into());
                };
                if !v.is_finite() {
                    return Err("listener.event.pos must be finite".into());
                }
                p[i] = *v;
            }
            p
        }
        _ => return Err("listener.event.pos must contain three doubles".into()),
    };
    let read_uuid = |name: &str| -> Result<Option<uuid::Uuid>, String> {
        match field(e, name) {
            None => Ok(None),
            Some(Tag::IntArray(words)) if words.len() == 4 => {
                let mut b = [0; 16];
                for (i, w) in words.iter().enumerate() {
                    b[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
                }
                Ok(Some(uuid::Uuid::from_bytes(b)))
            }
            _ => Err(format!("listener.event.{name} must be a four-int UUID")),
        }
    };
    let source_uuid = read_uuid("source")?;
    let projectile_owner_uuid = read_uuid("projectile_owner")?;
    Ok(Some(Pending {
        vibration: Vibration {
            event,
            pos,
            source: None,
            frequency: mc_data::game_events::frequency(event.name()),
            distance,
            source_uuid,
            projectile_owner_uuid,
        },
        arrival: tick.saturating_add(delay),
        selected: tick.saturating_sub(1),
    }))
}
#[cfg(test)]
#[path = "vibration_tests.rs"]
mod tests;
