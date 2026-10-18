const MECHANIC_DAMAGE_CAP: usize = 65536;

#[derive(Serialize)]
struct MechanicDamageTrace {
    bot: u32,
    window: Option<String>,
    entity_id: i32,
    target_uuid: Option<Uuid>,
    victim_is_self: bool,
    source_type_id: i32,
    source_direct_id: i32,
    source_cause_id: i32,
    direct_uuid: Option<Uuid>,
    cause_uuid: Option<Uuid>,
    direct_kind: Option<String>,
    cause_kind: Option<String>,
    source_position: Option<[f64; 3]>,
}

#[derive(Serialize)]
struct MechanicProjectileHit {
    projectile_uuid: Uuid,
    owner_uuid: Uuid,
    target_bot: u32,
    window: Option<String>,
    source_type_id: i32,
}
impl Mechanics {
    fn damage_event(
        &mut self,
        bot: u32,
        player_entity_id: i32,
        packet: &ClientboundDamageEvent,
        window: Option<&str>,
    ) {
        if window.is_none() {
            return;
        }
        self.damage_events_received += 1;
        let direct = self.wire_ids.get(&packet.source_direct_id).copied();
        let cause = self.wire_ids.get(&packet.source_cause_id).copied();
        let target = self.wire_ids.get(&packet.entity_id).copied();
        let victim_is_self = packet.entity_id == player_entity_id;
        let primary_victim = target
            .and_then(|uuid| self.entities.get(&uuid))
            .is_some_and(|entity| entity.observer_bot == bot && entity.visible_to(bot));
        if !victim_is_self && !primary_victim {
            self.damage_events_filtered += 1;
            return;
        }
        self.receipt(window, "damage_event");
        if (packet.source_direct_id >= 0 && direct.is_none())
            || (packet.source_cause_id >= 0 && cause.is_none())
        {
            self.damage_events_unresolved += 1;
        }
        if self.damage_events.len() >= MECHANIC_DAMAGE_CAP {
            self.damage_events_truncated += 1;
            return;
        }
        let direct_kind = direct
            .and_then(|uuid| self.entities.get(&uuid))
            .map(|entity| entity.kind.as_str());
        let cause_kind = cause
            .and_then(|uuid| self.entities.get(&uuid))
            .map(|entity| entity.kind.as_str());
        self.damage_events.push(MechanicDamageTrace {
            bot,
            window: window.map(str::to_owned),
            entity_id: packet.entity_id,
            target_uuid: target,
            victim_is_self,
            source_type_id: packet.source_type_id,
            source_direct_id: packet.source_direct_id,
            source_cause_id: packet.source_cause_id,
            direct_uuid: direct,
            cause_uuid: cause,
            direct_kind: direct_kind.map(str::to_owned),
            cause_kind: cause_kind.map(str::to_owned),
            source_position: packet.source_position,
        });
        if packet.entity_id == player_entity_id
            && matches!(direct_kind, Some("minecraft:arrow" | "minecraft:snowball"))
            && let (Some(projectile_uuid), Some(owner_uuid)) = (direct, cause) {
                if self.projectile_hit_keys.insert((projectile_uuid, bot)) {
                    self.projectile_hits.push(MechanicProjectileHit {
                        projectile_uuid,
                        owner_uuid,
                        target_bot: bot,
                        window: window.map(str::to_owned),
                        source_type_id: packet.source_type_id,
                    });
                } else {
                    self.projectile_duplicate_hits += 1;
                }
            }
    }
}
