//! Entity removal (swap-remove follow-up) and mob spawning.
//! Moved verbatim from `tick.rs`.

use super::*;

impl TickState {
    /// Remove an entity by slot; fixes connection/inventory/health maps.
    pub fn remove_entity(&mut self, slot: EntitySlot) {
        let body = self.entities.body(slot);
        let id = body.id;
        let kind = body.kind;
        let movement = body.movement;
        if self.entities.row(slot).projectile.is_some()
            || movement == crate::blueprint::MovementClass::Projectile
        {
            let uuid = self.entities.row(slot).uuid;
            crate::physical_attacks::drop_projectile(self, uuid);
        }
        self.armor_stand_hits.remove(&id);
        if let EntityKind::Player { .. } = kind
            && let Some(connection) = self.connections.by_entity.get(&id).copied()
        {
            self.connections.by_connection.remove(&connection);
            self.connections.by_entity.remove(&id);
            self.connections.view_distances.remove(&connection);
            self.connections.locales.remove(&connection);
            self.outbox.remove(&connection);
        }
        self.players.remove(&id);
        let last = self.entities.len() as u32 - 1;
        self.entities.remove(slot);
        self.follow_swap_removal(slot, last);
    }

    /// Swap-removal moved the row at `last` into `removed`. Slots staged for
    /// the merge/commit (damage, pickups, container custody) follow that row;
    /// entries for the removed row itself are dropped, so no staged slot can
    /// point past the store or at an unrelated entity.
    pub(super) fn follow_swap_removal(&mut self, removed: EntitySlot, last: u32) {
        let follow = |slot: EntitySlot| {
            if slot == removed {
                None
            } else if slot.0 == last {
                Some(removed)
            } else {
                Some(slot)
            }
        };
        self.merge_damage.retain_mut(|(slot, _)| {
            follow(*slot).is_some_and(|next| {
                *slot = next;
                true
            })
        });
        self.plugin_damage.retain_mut(|(slot, _)| {
            follow(*slot).is_some_and(|next| {
                *slot = next;
                true
            })
        });
        self.merge_pickups.retain_mut(|(item, collector, _)| {
            let (Some(next_item), Some(next_collector)) = (follow(*item), follow(*collector))
            else {
                return false;
            };
            *item = next_item;
            *collector = next_collector;
            true
        });
        if !self.merge_consumed_items.is_empty() {
            self.merge_consumed_items = self
                .merge_consumed_items
                .drain()
                .filter_map(follow)
                .collect();
        }
    }

    /// Spawn a mob/item entity from a blueprint type id.
    pub fn spawn_mob(&mut self, type_id: u32, pos: [f64; 3], health: f32) -> EntityId {
        let id = self.entities.alloc_id();
        let uuid = self.entity_uuid(id);
        let blueprint = self.registry.blueprint(type_id);
        let (movement, half_width, height, speed, damage, sight, reach) = blueprint
            .map(|blueprint| {
                (
                    blueprint.movement,
                    blueprint.half_width,
                    blueprint.height,
                    blueprint.params.speed,
                    blueprint.params.attack_damage,
                    blueprint.sensors.sight_range,
                    blueprint.sensors.attack_reach,
                )
            })
            .unwrap_or((
                crate::blueprint::MovementClass::Walker,
                0.3,
                1.8,
                2.0,
                0.0,
                10.0,
                2.0,
            ));
        let is_item = type_id == self.registry.item_type_id;
        let kind = if is_item {
            EntityKind::Item { type_id }
        } else {
            EntityKind::Mob { type_id }
        };
        let mut appearance = EntityAppearance {
            health,
            ..Default::default()
        };
        if self
            .registry
            .blueprint(type_id)
            .is_some_and(|blueprint| blueprint.wool)
        {
            let roll = uuid.as_u128();
            let color = crate::blueprint::natural_wool_color(roll as u64, (roll >> 64) as u64);
            appearance.extra.push((
                crate::blueprint::SHEEP_WOOL_DATA_INDEX,
                crate::boundary::EntityDataValue::Byte(color as i8),
            ));
        }
        let appearance = Arc::new(appearance);
        self.entities.spawn(
            id,
            uuid,
            kind,
            ColdFields {
                pos,
                vel: [0.0; 3],
                yaw: 0.0,
                pitch: 0.0,
                ground: false,
                health,
                // Adults start without a cooldown; birth initializes child age
                // separately through the native lifecycle module.
                breed_cooldown: 0,
                pickup_delay: if is_item {
                    self.registry
                        .blueprint(type_id)
                        .map(|blueprint| blueprint.params.pickup_delay)
                        .unwrap_or(10)
                } else {
                    0
                },
                item: None,
                appearance,
                movement,
                half_width,
                height,
                speed,
                attack_damage: damage,
                sight_range: sight,
                attack_reach: reach,
            },
        );
        if let Some(slot) = self.entities.slot_of(id) {
            crate::villager::conversion::initialize(self, slot, type_id);
            crate::entity_loot::initialize(self, slot);
            crate::tnt::initialize(self, slot, type_id);
            crate::falling::initialize(self, slot, type_id);
            crate::vehicles::initialize(self, slot);
            crate::special_mobs::initialize(self, slot, type_id);
            crate::bees::initialize(self, slot, type_id);
            crate::allay::initialize(self, slot, type_id);
            crate::copper_golem::initialize(self, slot, type_id);
            crate::mob_lanes::initialize(self, slot, type_id);
            crate::leash::initialize(self, slot);
            crate::end_fight::initialize_entity(self, slot);
            crate::dolphin::initialize(self, slot, type_id);
            crate::mob_equipment::initialize(self, slot, type_id);
        }
        id
    }
}
