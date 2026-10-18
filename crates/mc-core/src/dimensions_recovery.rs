use crate::config::CoreError;
use crate::journal::ReplayEntity;
use crate::tick::TickState;

/// Recovery rejects invalid retained images before a checkpoint can rewrite them.
/// A skipped record would otherwise become permanent data loss.
pub(crate) fn validate_entity(
    state: &TickState,
    id: i32,
    record: &ReplayEntity,
) -> Result<uuid::Uuid, CoreError> {
    let invalid = |reason: String| {
        CoreError::Storage(format!(
            "{}: journal entity wire-id {id} UUID {} type-id {}: {reason}",
            state.dimension.name(),
            record.uuid,
            record.type_id,
        ))
    };
    if !record.state.opaque_entity_fields.is_empty() {
        let mut bytes = record.state.opaque_entity_fields.as_slice();
        let (_, fields) = mc_nbt::read_named(&mut bytes)
            .map_err(|err| invalid(format!("opaque entity fields: {err}")))?;
        if !matches!(fields, mc_nbt::Tag::Compound(_)) || !bytes.is_empty() {
            return Err(invalid(
                "opaque entity fields: expected one complete named compound".into(),
            ));
        }
    }
    let uuid = record
        .uuid
        .parse::<uuid::Uuid>()
        .map_err(|err| invalid(format!("invalid UUID: {err}")))?;
    let contract =
        mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(record.type_id)
            .ok_or_else(|| invalid("unknown entity type-id".into()))?;
    if record.state.bee.is_some() && contract.name != "minecraft:bee" {
        return Err(invalid(
            "Bee: retained state incompatible with entity type".into(),
        ));
    }
    if let Some(allay) = &record.state.allay {
        if contract.name != "minecraft:allay" {
            return Err(invalid(
                "Allay: retained state incompatible with entity type".into(),
            ));
        }
        let rules = state.registry.item_rules();
        for stack in [&allay.held, &allay.inventory] {
            if !stack.is_empty()
                && (state.registry.items.name_of(stack.item_id).is_none()
                    || stack.count > rules.max_stack(stack))
            {
                return Err(invalid("Allay.Inventory: invalid registered stack".into()));
            }
        }
    }
    if record
        .pos
        .iter()
        .chain(record.vel.iter())
        .any(|value| !value.is_finite())
        || !record.yaw.is_finite()
        || !record.pitch.is_finite()
        || !record.health.is_finite()
        || !record.appearance.health.is_finite()
    {
        return Err(invalid(
            "non-finite position, motion, rotation or health".into(),
        ));
    }
    if let Some(projectile) = &record.projectile {
        crate::projectiles::persistence::validate_state(projectile)
            .map_err(|reason| invalid(reason.into()))?;
        if !projectile.stack.is_empty()
            && state
                .registry
                .items
                .name_of(projectile.stack.item_id)
                .is_none()
        {
            return Err(invalid(format!(
                "projectile has unknown item-id {}",
                projectile.stack.item_id
            )));
        }
        if contract.name != projectile.kind.name() {
            return Err(invalid(format!(
                "projectile kind {} disagrees with entity type {}",
                projectile.kind.name(),
                contract.name
            )));
        }
    }
    Ok(uuid)
}

/// Hydrate one native entity compound without filesystem access.
pub(crate) fn hydrate_entity(
    registry: &crate::registry::CoreRegistry,
    base: &mut crate::journal::Replay,
    payload: &mc_nbt::Tag,
    manifest_tick: u64,
) -> Result<(), CoreError> {
    let mc_nbt::Tag::Compound(fields) = payload else {
        return Err(CoreError::Storage(
            "native entity: expected compound".into(),
        ));
    };
    mc_world::anvil::validate_shared_fields(fields)
        .map_err(|error| CoreError::Storage(format!("native entity: {error}")))?;
    let empty = mc_data::entity_nbt::VanillaEntity {
        id: String::new(),
        uuid: uuid::Uuid::nil(),
        pos: [0.0; 3],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        health: 20.0,
        age: 0,
        breed_cooldown: 0,
        pickup_delay: 0,
        item: None,
        experience_orb: None,
        sheep_wool: None,
        active_effects: Vec::new(),
        absorption_amount: 0.0,
        wire_id: 0,
        extra_fields: Vec::new(),
    };
    let record = mc_data::entity_nbt::entity_from_fields(&registry.items, empty, fields).map_err(
        |error| {
            use mc_data::item_nbt::ItemNbtError;
            let field = match &error {
                ItemNbtError::InvalidId(_) | ItemNbtError::UnknownItem(_) => "Item.id",
                ItemNbtError::InvalidEnchantment(_) => "Item.components.minecraft:enchantments",
                ItemNbtError::InvalidComponent(_) => "Item.components",
            };
            CoreError::Storage(format!("native entity {field}: {error}"))
        },
    )?;
    let invalid =
        |reason: String| CoreError::Storage(format!("native entity {}: {reason}", record.uuid));
    let type_id = registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(record.id.clone()).map_err(|e| invalid(e.to_string()))?)
        .ok_or_else(|| invalid("unknown type".into()))?;
    let wire_id = record.wire_id;
    if wire_id <= 0 || base.entities.contains_key(&wire_id) {
        return Err(invalid(format!("invalid or duplicate WireId {wire_id}")));
    }
    use crate::core::{appearance_from_vanilla_entity, velocity_from_vanilla_motion};
    if record
        .pos
        .iter()
        .chain(record.motion.iter())
        .any(|value| !value.is_finite())
        || !record.yaw.is_finite()
        || !record.pitch.is_finite()
        || !record.health.is_finite()
    {
        return Err(invalid(
            "non-finite position, motion, rotation or health".into(),
        ));
    }
    let projectile = if crate::projectiles::ProjectileKind::from_name(&record.id).is_some() {
        Some(
            crate::projectiles::persistence::from_entity(&record, &registry.items)
                .map_err(&invalid)?,
        )
    } else {
        None
    };
    let ageable = registry.blueprint(type_id).is_some_and(|blueprint| {
        blueprint
            .goals
            .iter()
            .any(|goal| goal.kind == crate::blueprint::GoalKind::Breed)
    });
    let mut appearance = appearance_from_vanilla_entity(&record);
    crate::end_fight::native_appearance(&record, &mut appearance).map_err(&invalid)?;
    if let Some(fight) = &mut base.end_fight {
        crate::end_fight::hydrate_native_entity(fight, &record).map_err(&invalid)?;
    }
    appearance.equipment =
        crate::entity_loot::equipment_from_fields(&registry.items, &record.extra_fields)?.into();
    let native_state = crate::journal::EntityStateImage::from_record(registry, &record)
        .map_err(|err| invalid(err.to_string()))?;
    appearance.baby |= native_state.entity_life.child_ticks != 0;
    appearance.shared_flags |= u8::from(native_state.burn_ticks != 0);
    base.entities.insert(
        wire_id,
        crate::journal::ReplayEntity {
            uuid: record.uuid.to_string(),
            type_id,
            pos: record.pos,
            vel: velocity_from_vanilla_motion(record.motion),
            yaw: record.yaw,
            pitch: record.pitch,
            ground: record.on_ground,
            health: record.health,
            alive: true,
            age: native_state.age,
            breed_cooldown: record
                .breed_cooldown
                .max(if ageable { record.age.max(0) } else { 0 })
                as u32,
            pickup_delay: record.pickup_delay as u32,
            item: record.item.clone(),
            appearance,
            active_effects: crate::journal::active_effects_from_nbt(
                &record.active_effects,
                record.absorption_amount,
            ),
            villager: if matches!(
                record.id.as_str(),
                "minecraft:villager" | "minecraft:zombie_villager" | "minecraft:wandering_trader"
            ) {
                let mut state = crate::villager::persistence::villager_from_fields(
                    &registry.items,
                    &record.extra_fields,
                    manifest_tick,
                )
                .map_err(|err| CoreError::Storage(format!("villager {}: {err}", record.uuid)))?;
                state.wandering = record.id == "minecraft:wandering_trader";
                if record.id.as_str() == "minecraft:zombie_villager"
                    && state.conversion_time.is_none()
                {
                    state.conversion_time = Some(-1);
                }
                Some(state)
            } else {
                None
            },
            state: native_state,
            projectile,
            tick: manifest_tick,
        },
    );
    Ok(())
}
