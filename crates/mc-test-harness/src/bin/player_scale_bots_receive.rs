// Packet-driven images, menu ledgers and lifecycle evidence for mechanic probes.
impl std::fmt::Debug for Mechanics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mechanics")
            .field("spec", &self.spec)
            .finish_non_exhaustive()
    }
}
impl Mechanics {
    fn record_menu_version(menu: &mut MenuReceipt, version: i32) {
        if menu.versions.last() != Some(&version) {
            if menu.versions.len() == 256 {
                menu.versions.remove(1);
                menu.versions_truncated += 1;
            }
            menu.versions.push(version);
        }
    }
    fn menu_image(
        &mut self,
        index: usize,
        version: i32,
        items: &[ItemStack],
        carried: &ItemStack,
        window: Option<&str>,
    ) -> Result<()> {
        ensure!(
            items.len() <= MECHANIC_SLOT_CAP && items.len() >= 36,
            "unexpected menu image slot count"
        );
        let count = self.spec.containers[index]
            .slots
            .unwrap_or(items.len() - 36);
        ensure!(
            items.len() == count + 36,
            "menu slot count does not match declared container"
        );
        let slots: Vec<_> = items[..count]
            .iter()
            .enumerate()
            .map(|(i, item)| self.slot(i, item))
            .collect();
        let inventory: Vec<_> = items[count..]
            .iter()
            .enumerate()
            .map(|(i, item)| self.slot(count + i, item))
            .collect();
        let carried = self.slot(usize::MAX, carried);
        let menu = &mut self.menus[index];
        let container_changed = menu
            .final_slots
            .as_ref()
            .is_some_and(|before| before != &slots);
        let changed = container_changed
            || menu
                .final_player_inventory
                .as_ref()
                .is_some_and(|before| before != &inventory)
            || menu
                .carried
                .as_ref()
                .is_some_and(|before| before != &carried);
        if menu.initial.is_none() || window.is_none() {
            menu.initial = Some(slots.clone());
            menu.initial_player_inventory = Some(inventory.clone());
        }
        menu.final_slots = Some(slots);
        menu.final_player_inventory = Some(inventory);
        menu.carried = Some(carried);
        menu.content_packets += 1;
        if changed && window.is_some() {
            menu.updates += 1;
            menu.container_slot_updates += u64::from(container_changed);
        }
        Self::record_menu_version(menu, version);
        if let Some(window) = window {
            *menu
                .received_by_window
                .entry(window.to_owned())
                .or_default() += 1;
        }
        self.receipt(window, "container_image");
        if changed {
            self.receipt(window, "container_update");
        }
        Ok(())
    }
    fn menu_slot(
        &mut self,
        index: usize,
        version: i32,
        slot: i16,
        stack: &ItemStack,
        window: Option<&str>,
    ) -> Result<()> {
        if slot == -1 {
            let image = self.slot(usize::MAX, stack);
            let changed = self.menus[index]
                .carried
                .as_ref()
                .is_some_and(|before| before != &image);
            self.menus[index].carried = Some(image);
            if changed && window.is_some() {
                self.menus[index].updates += 1;
                self.receipt(window, "container_update");
            }
            return Ok(());
        }
        let slot = usize::try_from(slot)?;
        ensure!(slot < MECHANIC_SLOT_CAP, "menu slot exceeds probe budget");
        let image = self.slot(slot, stack);
        let menu = &mut self.menus[index];
        let Some(container) = menu.final_slots.as_mut() else {
            return Ok(());
        };
        let changed = if slot < container.len() {
            let changed = container[slot] != image;
            container[slot] = image;
            changed
        } else {
            let player_slot = slot - container.len();
            let inventory = menu
                .final_player_inventory
                .as_mut()
                .context("menu missing player image")?;
            ensure!(
                player_slot < inventory.len(),
                "menu slot outside received image"
            );
            let changed = inventory[player_slot] != image;
            inventory[player_slot] = image;
            changed
        };
        let container_changed = slot < container.len() && changed;
        Self::record_menu_version(menu, version);
        if changed && window.is_some() {
            menu.updates += 1;
            menu.container_slot_updates += u64::from(container_changed);
        }
        if window.is_none() {
            menu.initial.clone_from(&menu.final_slots);
            menu.initial_player_inventory
                .clone_from(&menu.final_player_inventory);
        }
        if let Some(window) = window {
            *menu
                .received_by_window
                .entry(window.to_owned())
                .or_default() += 1;
        }
        if changed {
            self.receipt(window, "container_update");
        }
        Ok(())
    }
}
impl Bot {
    fn mechanic_frame(&mut self, frame: &RawFrame, window: Option<&str>) -> Result<()> {
        let Some(shared) = self.mechanics.as_ref().map(Arc::clone) else {
            return Ok(());
        };
        if !matches!(
            frame.id,
            AddEntity::ID
                | EntityEvent::ID
                | ClientboundDamageEvent::ID
                | MoveEntityPos::ID
                | MoveEntityPosRot::ID
                | EntityPositionSync::ID
                | RemoveEntities::ID
                | ClientboundSetEntityData::ID
                | ClientboundOpenScreen::ID
                | ClientboundContainerSetContent::ID
                | ClientboundContainerSetSlot::ID
                | ClientboundContainerSetData::ID
                | ClientboundContainerClose::ID
                | ClientboundSetHealth::ID
                | ClientboundRespawn::ID
                | GameEvent::ID
                | ClientboundExplode::ID
                | ClientboundSystemChat::ID
        ) {
            return Ok(());
        }
        let mut body = frame.body.as_ref();
        let mut state = shared.lock();
        if !state.accepts_receive(window) {
            return Ok(());
        }
        match frame.id {
            ClientboundDamageEvent::ID => {
                let packet = ClientboundDamageEvent::decode(&mut body)?;
                ensure!(
                    packet.source_position.is_none_or(|position| position
                        .iter()
                        .all(|coordinate| coordinate.is_finite())),
                    "nonfinite damage source position"
                );
                state.damage_event(self.index, self.entity_id, &packet, window);
            }
            AddEntity::ID => {
                let packet = AddEntity::decode(&mut body)?;
                let new = !state.entities.contains_key(&packet.uuid);
                state.spawn(self.index, &packet, window);
                if new
                    && state.entities.contains_key(&packet.uuid)
                    && self
                        .facts
                        .projectile_type_ids
                        .contains(&packet.entity_type_id)
                {
                    state.receipt(window, "projectile_spawn");
                }
            }
            EntityEvent::ID => {
                let packet = EntityEvent::decode(&mut body)?;
                if packet.event_id == 3
                    && let Some(uuid) = state.wire_ids.get(&packet.entity_id).copied()
                {
                    let entity = state.entities.get_mut(&uuid).expect("indexed entity");
                    if !entity.visible_to(self.index) {
                        state.ghost_updates += 1;
                        return Ok(());
                    }
                    if !entity.dead {
                        entity.dead = true;
                        entity.death_events += u64::from(window.is_some());
                        state.receipt(window, "entity_death");
                    }
                }
            }
            MoveEntityPos::ID => {
                let packet = MoveEntityPos::decode(&mut body)?;
                state.movement(
                    self.index,
                    packet.entity_id,
                    [
                        f64::from(packet.delta_x) / 4096.0,
                        f64::from(packet.delta_y) / 4096.0,
                        f64::from(packet.delta_z) / 4096.0,
                    ],
                    true,
                    window,
                );
            }
            MoveEntityPosRot::ID => {
                let packet = MoveEntityPosRot::decode(&mut body)?;
                state.movement(
                    self.index,
                    packet.entity_id,
                    [
                        f64::from(packet.delta_x) / 4096.0,
                        f64::from(packet.delta_y) / 4096.0,
                        f64::from(packet.delta_z) / 4096.0,
                    ],
                    true,
                    window,
                );
            }
            EntityPositionSync::ID => {
                let packet = EntityPositionSync::decode(&mut body)?;
                let p = packet.position;
                state.movement(self.index, packet.entity_id, [p.x, p.y, p.z], false, window);
            }
            RemoveEntities::ID => {
                let packet = RemoveEntities::decode(&mut body)?;
                for id in packet.entity_ids {
                    state.remove(self.index, id, window);
                }
            }
            ClientboundSetEntityData::ID => {
                let packet = ClientboundSetEntityData::decode(&mut body)?;
                if let Some(uuid) = state.wire_ids.get(&packet.entity_id).copied() {
                    let mut item = None;
                    for value in &packet.values {
                        if let EntityDataValue::ItemStack { stack, .. } = value {
                            item = Some(state.slot(0, stack));
                        }
                    }
                    let entity = state.entities.get_mut(&uuid).expect("indexed entity");
                    if !entity.visible_to(self.index) {
                        state.ghost_updates += 1;
                        return Ok(());
                    }
                    if entity.observer_bot != self.index {
                        return Ok(());
                    }
                    let mut changed = false;
                    for value in packet.values {
                        let index = value.index();
                        let image = match value {
                            EntityDataValue::Float { value, .. } => {
                                if index == LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2 {
                                    if value <= 0.0 && !entity.dead {
                                        entity.death_events += 1;
                                    }
                                    entity.dead = value <= 0.0;
                                    entity.health = Some(value);
                                }
                                json!({"float":value})
                            }
                            EntityDataValue::Byte { value, .. } => {
                                if index == 0 {
                                    entity.flags = Some(value);
                                }
                                json!({"byte":value})
                            }
                            EntityDataValue::Pose { pose, .. } => {
                                entity.pose = Some(format!("{pose:?}"));
                                json!({"pose":entity.pose})
                            }
                            EntityDataValue::VillagerData {
                                villager_type,
                                profession,
                                level,
                                ..
                            } => {
                                json!({"villager_type":villager_type,"profession":profession,"level":level})
                            }
                            EntityDataValue::BlockPosition { value, .. } => {
                                json!({"position":unpack_block_pos(value)})
                            }
                            EntityDataValue::OptionalBlockPosition { value, .. } => {
                                json!({"position":value.map(unpack_block_pos)})
                            }
                            EntityDataValue::Boolean { value, .. } => json!({"bool":value}),
                            EntityDataValue::Int { value, .. } => json!({"int":value}),
                            EntityDataValue::ItemStack { .. } => {
                                entity.item = item.clone();
                                json!({"item":item})
                            }
                            _ => continue,
                        };
                        if entity.metadata.get(&index) != Some(&image) {
                            entity.metadata.insert(index, image);
                            changed = true;
                        }
                    }
                    if changed {
                        entity.metadata_updates += u64::from(window.is_some());
                        state.receipt(window, "metadata_change");
                    }
                }
            }
            ClientboundOpenScreen::ID => {
                let packet = ClientboundOpenScreen::decode(&mut body)?;
                state.players[self.index as usize].menu_opens += u64::from(window.is_some());
                if let Some(index) = self.mechanic_bot.opening.take() {
                    self.mechanic_bot.menu = Some((packet.container_id, index, 0));
                    state.receipt(window, "menu_open");
                } else {
                    state.receipt(window, "unassigned_menu_open");
                }
            }
            ClientboundContainerSetContent::ID => {
                let packet = ClientboundContainerSetContent::decode(&mut body)?;
                ensure!(
                    packet.items.len() <= MECHANIC_SLOT_CAP,
                    "inventory exceeds bounded probe capacity"
                );
                if packet.container_id == 0 {
                    let images: Vec<_> = packet
                        .items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| state.slot(i, item))
                        .collect();
                    let player = &mut state.players[self.index as usize];
                    if player.initial_inventory.is_none() || window.is_none() {
                        player.initial_inventory = Some(images.clone());
                    }
                    if player
                        .final_inventory
                        .as_ref()
                        .is_some_and(|before| before != &images)
                    {
                        player.inventory_updates += u64::from(window.is_some());
                    }
                    player.final_inventory = Some(images);
                    state.receipt(window, "inventory_image");
                } else if let Some((id, index, _)) = self.mechanic_bot.menu
                    && id == packet.container_id
                {
                    state.menu_image(
                        index,
                        packet.state_id,
                        &packet.items,
                        &packet.carried_item,
                        window,
                    )?;
                    let count = packet.items.len() - 36;
                    for (slot, item) in packet.items[count..].iter().enumerate() {
                        let slot = slot + 9;
                        let image = state.slot(slot, item);
                        let player = &mut state.players[self.index as usize];
                        if let Some(inventory) = player.final_inventory.as_mut() {
                            ensure!(
                                inventory.len() >= 45,
                                "player menu projection requires fresh complete player inventory"
                            );
                            if inventory[slot] != image {
                                player.inventory_updates += u64::from(window.is_some());
                                inventory[slot] = image;
                            }
                        }
                    }
                    if window.is_none() {
                        let player = &mut state.players[self.index as usize];
                        player.initial_inventory.clone_from(&player.final_inventory);
                    }
                    self.mechanic_bot.menu = Some((id, index, packet.state_id));
                }
            }
            ClientboundContainerSetSlot::ID => {
                let packet = ClientboundContainerSetSlot::decode(&mut body)?;
                if packet.container_id == 0 || packet.container_id == -2 {
                    let slot = usize::try_from(packet.slot)?;
                    ensure!(
                        slot < MECHANIC_SLOT_CAP,
                        "inventory slot exceeds probe capacity"
                    );
                    let image = state.slot(slot, &packet.item_stack);
                    let player = &mut state.players[self.index as usize];
                    if let Some(inventory) = player.final_inventory.as_mut() {
                        ensure!(
                            slot < inventory.len(),
                            "inventory slot outside received image"
                        );
                        if inventory[slot] != image {
                            player.inventory_updates += u64::from(window.is_some());
                            inventory[slot] = image;
                        }
                    }
                    if window.is_none() {
                        player.initial_inventory.clone_from(&player.final_inventory);
                    }
                    state.receipt(window, "inventory_update");
                } else if let Some((id, index, _)) = self.mechanic_bot.menu
                    && (id == packet.container_id || packet.container_id == -1)
                {
                    state.menu_slot(
                        index,
                        packet.state_id,
                        packet.slot,
                        &packet.item_stack,
                        window,
                    )?;
                    let container_slots =
                        state.menus[index].final_slots.as_ref().map_or(0, Vec::len);
                    if packet.slot >= container_slots as i16 {
                        let slot = 9 + packet.slot as usize - container_slots;
                        let image = state.slot(slot, &packet.item_stack);
                        let player = &mut state.players[self.index as usize];
                        if let Some(inventory) = player.final_inventory.as_mut() {
                            ensure!(
                                slot < inventory.len(),
                                "player slot projection outside fresh inventory"
                            );
                            if inventory[slot] != image {
                                player.inventory_updates += u64::from(window.is_some());
                                inventory[slot] = image;
                            }
                        }
                        if window.is_none() {
                            player.initial_inventory.clone_from(&player.final_inventory);
                        }
                    }
                    self.mechanic_bot.menu = Some((id, index, packet.state_id));
                }
            }
            ClientboundContainerSetData::ID => {
                let packet = ClientboundContainerSetData::decode(&mut body)?;
                if let Some((id, index, _)) = self.mechanic_bot.menu
                    && id == packet.container_id
                {
                    state.menus[index]
                        .properties
                        .insert(packet.id, packet.value);
                    state.receipt(window, "container_property");
                }
            }
            ClientboundContainerClose::ID => {
                let packet = ClientboundContainerClose::decode(&mut body)?;
                if self
                    .mechanic_bot
                    .menu
                    .is_some_and(|(id, _, _)| id == packet.container_id)
                {
                    self.mechanic_bot.menu = None;
                }
                state.receipt(window, "menu_close");
            }
            ClientboundSetHealth::ID => {
                let packet = ClientboundSetHealth::decode(&mut body)?;
                ensure!(packet.health.is_finite(), "nonfinite received health");
                ensure!(
                    packet.saturation.is_finite(),
                    "nonfinite received saturation"
                );
                state.nutrition(self.index, packet.food, packet.saturation, window);
                let player = &mut state.players[self.index as usize];
                let before = player.final_health;
                if player.initial_health.is_none() || window.is_none() {
                    player.initial_health = Some(packet.health);
                }
                let damaged =
                    window.is_some() && before.is_some_and(|health| packet.health < health);
                let died = window.is_some()
                    && packet.health <= 0.0
                    && before.is_none_or(|health| health > 0.0);
                if damaged {
                    player.damage_events += 1;
                    player.damage_amount +=
                        f64::from(before.expect("prior health") - packet.health);
                }
                if died {
                    player.deaths += 1;
                }
                player.final_health = Some(packet.health);
                if let Some(window) = window {
                    *player
                        .received_by_window
                        .entry(window.to_owned())
                        .or_default() += 1;
                }
                if damaged {
                    state.receipt(window, "health_damage");
                }
                if died {
                    state.receipt(window, "death");
                }
            }
            ClientboundRespawn::ID => {
                let player = &mut state.players[self.index as usize];
                if window.is_some() {
                    player.respawns += 1;
                }
                player.world_epoch += 1;
                player.final_health = None;
                player.final_food = None;
                player.final_saturation = None;
                for entity in state.entities.values_mut() {
                    entity.hide(self.index);
                }
                state.receipt(window, "respawn");
                self.surfaces.clear();
                self.mechanic_bot.menu = None;
                self.mechanic_bot.opening = None;
            }
            GameEvent::ID => {
                let packet = GameEvent::decode(&mut body)?;
                if packet.event == GameEvent::EVENT_CHANGE_GAME_MODE {
                    let player = &mut state.players[self.index as usize];
                    player.game_mode = Some(packet.value as i32);
                    if packet.value == 0.0 && window.is_some() {
                        player.survival_mode_receipts += 1;
                    }
                    state.receipt(window, "game_mode");
                }
            }
            ClientboundExplode::ID => {
                let packet = ClientboundExplode::decode(&mut body)?;
                let p = [packet.center.x, packet.center.y, packet.center.z];
                ensure!(
                    p.iter().all(|v| v.is_finite()) && packet.radius.is_finite(),
                    "nonfinite explosion"
                );
                if self.index == 0 && window.is_some() && state.explosions.len() < 256 {
                    state.explosions.push(json!({"bot":self.index,"window":window,"center":p,"radius":packet.radius,"block_count":packet.block_count,"knockback":packet.knockback.map(|p|[p.x,p.y,p.z])}));
                }
                state.receipt(window, "explosion");
            }
            ClientboundSystemChat::ID => {
                if !state.command_observers.contains(&self.index) {
                    return Ok(());
                }
                let packet = ClientboundSystemChat::decode(&mut body)?;
                let tag = mc_nbt::read_network(&mut packet.content_nbt.as_slice())?;
                state.record_reply(self.index, window, &tag);
                for pending in &self.mechanic_bot.pending {
                    if let MechanicExpectation::Feedback(expected) = &pending.expectation
                        && state.actions[pending.receipt].completed == 0
                        && window.is_some()
                        && literal_component_matches(&tag, expected)
                    {
                        let receipt = &mut state.actions[pending.receipt];
                        receipt.completed = 1;
                        receipt.completed_in_window = window.map(str::to_owned);
                        break;
                    }
                }
                if self.index == 0
                    && window.is_some()
                    && literal_component_matches(&tag, "Saved the game")
                {
                    state.save_completions += 1;
                    state.receipt(window, "save_completion");
                }
            }
            _ => {}
        }
        Ok(())
    }
    async fn prepare_mechanic_menus(&mut self, client: &mut Client, vd: i32) -> Result<()> {
        // A fresh read-only process has no cached menu data. It opens each assigned
        // reachable probe once and waits only for that actual content image.
        if !self.mechanic_read_only {
            return Ok(());
        }
        let Some(shared) = self.mechanics.as_ref().map(Arc::clone) else {
            return Ok(());
        };
        let probes: Vec<_> = {
            let state = shared.lock();
            state
                .spec
                .containers
                .iter()
                .enumerate()
                .filter(|(_, p)| p.bot == self.index)
                .map(|(i, p)| (i, p.position, p.name.clone()))
                .collect()
        };
        for (index, position, name) in probes {
            ensure!(
                position
                    .iter()
                    .map(|x| f64::from(*x) + 0.5)
                    .zip([self.x, self.y + 1.62, self.z])
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f64>()
                    <= 36.0,
                "read-only menu probe {name} is not reachable at persisted player position"
            );
            self.mechanic_bot.opening = Some(index);
            let before = shared.lock().menus[index].content_packets;
            self.mechanic_use_block(client, position).await?;
            tokio::time::timeout(ACTION_TIMEOUT, async {
                loop {
                    if shared.lock().menus[index].content_packets > before {
                        break;
                    }
                    let frame = client.read_frame().await?;
                    self.frame(client, frame, vd, true).await?;
                }
                Ok::<(), anyhow::Error>(())
            })
            .await
            .with_context(|| format!("fresh read-only content deadline for {name}"))??;
            let (id, _, _) = self
                .mechanic_bot
                .menu
                .take()
                .context("received menu disappeared before readback")?;
            client
                .write_packet(&ServerboundContainerClose { container_id: id })
                .await?;
        }
        Ok(())
    }
}
