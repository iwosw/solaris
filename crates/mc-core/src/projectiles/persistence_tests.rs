use super::*;
use crate::journal::{
    EntityAppearanceImage, JournalEntry, JournalWriter, Replay, ReplayEntity,
    read_complete_batches_after, replay_batches,
};

fn items() -> ItemRegistry {
    mc_data::items::solaris_required_items()
}
fn stack(items: &ItemRegistry, name: &str) -> ItemStack {
    ItemStack::new(
        items
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap(),
        1,
    )
}
fn type_id(kind: ProjectileKind) -> u32 {
    mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse(kind.name()).unwrap())
        .unwrap()
}
fn live_state() -> crate::tick::TickState {
    let registry = std::sync::Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let generator: std::sync::Arc<dyn mc_world::ChunkGenerator> = std::sync::Arc::new(
        mc_worldgen::TerrainGenerator::new(42, std::sync::Arc::clone(&registry.blocks)),
    );
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 64, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    crate::tick::TickState::new(registry, world, 2, 512)
}
fn image(state: &ProjectileState, items: &ItemRegistry) -> mc_data::entity_nbt::VanillaEntity {
    let mut fields = to_fields(state, items);
    fields.retain(|(key, _)| key != "Item");
    mc_data::entity_nbt::VanillaEntity {
        id: state.kind.name().into(),
        uuid: uuid::Uuid::from_u128(123),
        wire_id: 7,
        pos: [-3.0, 70.0, 6.0],
        motion: [0.25, 0.1, -1.5],
        age: state.age as i32,
        item: (item_key(state.kind) == Some("Item")).then(|| state.stack.clone()),
        extra_fields: fields,
        ..Default::default()
    }
}
fn replay_image(
    entity: &mc_data::entity_nbt::VanillaEntity,
    projectile: ProjectileState,
) -> ReplayEntity {
    ReplayEntity {
        uuid: entity.uuid.to_string(),
        type_id: type_id(projectile.kind),
        pos: entity.pos,
        vel: entity.motion.map(|value| value * 20.0),
        yaw: entity.yaw,
        pitch: entity.pitch,
        ground: entity.on_ground,
        health: entity.health,
        alive: true,
        age: entity.age as u32,
        breed_cooldown: 0,
        pickup_delay: 0,
        item: None,
        appearance: EntityAppearanceImage::default(),
        active_effects: Default::default(),
        villager: None,
        state: Default::default(),
        projectile: Some(projectile),
        tick: 10,
    }
}
fn entry(state: &ProjectileState, position: [f64; 3], velocity: [f64; 3]) -> JournalEntry {
    JournalEntry::Entity {
        id: 7,
        uuid: uuid::Uuid::from_u128(123).to_string(),
        type_id: type_id(state.kind),
        health: 1.0,
        alive: true,
        age: state.age,
        breed_cooldown: 0,
        pickup_delay: 0,
        item: None,
        appearance: EntityAppearanceImage::default(),
        active_effects: Default::default(),
        villager: None,
        state: Default::default(),
        projectile: Some(Box::new(state.clone())),
        tick: 11,
        pos: position,
        vel: velocity,
        yaw: 25.0,
        pitch: -10.0,
    }
}
fn world(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "solaris-projectiles-{label}-{}",
        uuid::Uuid::new_v4()
    ))
}

#[test]
fn in_flight_checkpoint_and_journal_reopen_preserve_motion_and_single_custody() {
    let items = items();
    let dir = world("flight");
    let mut projectile = ProjectileState::new(
        ProjectileKind::Trident,
        Some(uuid::Uuid::from_u128(9)),
        stack(&items, "minecraft:trident"),
    );
    projectile.pickup = true;
    projectile.stack.damage = Some(38);
    projectile.age = 12;
    let entity = image(&projectile, &items);
    let chunks = std::collections::BTreeMap::from([(
        (-1, 0),
        mc_world::anvil::EntityChunk {
            x: -1,
            z: 0,
            entities: vec![entity],
        },
    )]);
    mc_world::anvil::write_entity_regions(&dir.join("entities"), &items, &chunks, 10).unwrap();
    let checkpoint = mc_world::anvil::read_entity_regions(&dir.join("entities"), &items).unwrap();
    let saved = &checkpoint[&(-1, 0)].entities[0];
    let restored = from_entity(saved, &items).unwrap();
    assert_eq!(restored, projectile);
    let mut base = Replay::default();
    base.entities.insert(7, replay_image(saved, restored));
    projectile.age = 13;
    let pos = [-2.75, 70.1, 4.5];
    let vel = [4.95, 1.0, -29.7];
    {
        let mut writer = JournalWriter::open(&dir, 1).unwrap();
        writer
            .append(11, 11, &[entry(&projectile, pos, vel)])
            .unwrap();
        writer.flush().unwrap(); // Abrupt stop: no shutdown/checkpoint update.
    }
    let recovered = replay_batches(
        base,
        &read_complete_batches_after(&dir.join("journal"), 10).unwrap(),
    );
    let recovered = &recovered.entities[&7];
    assert_eq!(recovered.pos, pos);
    assert_eq!(recovered.vel, vel);
    assert_eq!(recovered.projectile.as_ref(), Some(&projectile));
    assert_eq!(recovered.projectile.as_ref().unwrap().stack.count, 1);
    let mut live = live_state();
    live.tick = recovered.tick;
    crate::core::restore_entity(&mut live, 7, recovered).unwrap();
    let slot = live.entities.slot_of(crate::boundary::EntityId(7)).unwrap();
    assert_eq!(live.entities.pos(slot), pos);
    assert_eq!(live.entities.vel(slot), vel);
    assert_eq!(
        live.entities.row(slot).projectile.as_deref(),
        Some(&projectile)
    );
    // Pickup/removal is a tombstone, so the checkpoint cannot resurrect ammo.
    {
        let mut writer = JournalWriter::open(&dir, 1).unwrap();
        writer
            .append(12, 12, &[JournalEntry::RemoveEntity { id: 7, tick: 12 }])
            .unwrap();
        writer.flush().unwrap();
    }
    let mut base = Replay::default();
    base.entities
        .insert(7, replay_image(saved, from_entity(saved, &items).unwrap()));
    let recovered = replay_batches(
        base,
        &read_complete_batches_after(&dir.join("journal"), 10).unwrap(),
    );
    assert!(!recovered.entities.contains_key(&7));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn journal_only_birth_keeps_launch_velocity_and_old_entries_default() {
    let items = items();
    let state = ProjectileState::new(
        ProjectileKind::Arrow,
        None,
        stack(&items, "minecraft:arrow"),
    );
    let entry = entry(&state, [1.0, 2.0, 3.0], [0.0, 5.0, 60.0]);
    let recovered = replay_batches(Replay::default(), &[(11, 11, vec![entry.clone()])]);
    assert_eq!(recovered.entities[&7].vel, [0.0, 5.0, 60.0]);
    let mut old = serde_json::to_value(entry).unwrap();
    assert_eq!(
        old["Entity"]["projectile"],
        serde_json::to_value(&state).unwrap()
    );
    let fields = old["Entity"].as_object_mut().unwrap();
    fields.remove("projectile");
    fields.remove("vel");
    let old: JournalEntry = serde_json::from_value(old).unwrap();
    let recovered = replay_batches(Replay::default(), &[(11, 11, vec![old])]);
    assert!(recovered.entities[&7].projectile.is_none());
    assert_eq!(recovered.entities[&7].vel, [0.0; 3]);
}

#[test]
fn retained_cloud_hit_ledger_and_item_components_roundtrip_actual_nbt() {
    let items = items();
    let potion_fields = vec![
        (
            "id".into(),
            Tag::String("minecraft:lingering_potion".into()),
        ),
        ("count".into(), Tag::Int(1)),
        (
            "components".into(),
            Tag::Compound(vec![
                (
                    "minecraft:potion_contents".into(),
                    Tag::Compound(vec![(
                        "potion".into(),
                        Tag::String("minecraft:poison".into()),
                    )]),
                ),
                (
                    "minecraft:custom_data".into(),
                    Tag::Compound(vec![("retained".into(), Tag::Int(99))]),
                ),
            ]),
        ),
    ];
    let stack = stack_from_fields(&items, &potion_fields).unwrap().unwrap();
    let mut state = ProjectileState::new(
        ProjectileKind::AreaEffectCloud,
        Some(uuid::Uuid::from_u128(999)),
        stack,
    );
    state.age = 32;
    state.hit.push(uuid::Uuid::from_u128(101));
    state.data = ProjectileData::Cloud(super::super::payloads::CloudState {
        radius: 2.75,
        waiting: false,
        dragon: false,
        duration: 600,
        wait_time: 10,
        reapplication_delay: 100,
        radius_on_use: -0.5,
        radius_per_tick: -0.005,
        next_tick: 120,
        expires_tick: 700,
        affected_until: std::collections::BTreeMap::from([(uuid::Uuid::from_u128(101), 150)]),
    });
    let entity = image(&state, &items);
    let fields = mc_data::entity_nbt::entity_to_fields(&items, &entity).unwrap();
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", &Tag::Compound(fields)).unwrap();
    let (_, Tag::Compound(fields)) = mc_nbt::read_named(&mut std::io::Cursor::new(bytes)).unwrap()
    else {
        panic!("entity root")
    };
    let entity =
        mc_data::entity_nbt::entity_from_fields(&items, Default::default(), &fields).unwrap();
    assert_eq!(from_entity(&entity, &items).unwrap(), state);
    assert_eq!(
        field(&entity.extra_fields, "Radius"),
        Some(&Tag::Float(2.75))
    );
    assert!(field(&entity.extra_fields, "potion_contents").is_some());
}

#[test]
fn vanilla_arrow_fields_are_authoritative_and_malicious_restore_fails_closed() {
    let items = items();
    let mut state = ProjectileState::new(
        ProjectileKind::Arrow,
        None,
        stack(&items, "minecraft:arrow"),
    );
    state.pickup = true;
    let mut fields = to_fields(&state, &items);
    assert_eq!(from_fields(&fields, &items), Ok(state.clone()));
    fields
        .iter_mut()
        .find(|(key, _)| key == "pickup")
        .unwrap()
        .1 = Tag::Byte(0);
    assert!(!from_fields(&fields, &items).unwrap().pickup);
    fields
        .iter_mut()
        .find(|(key, _)| key == "damage")
        .unwrap()
        .1 = Tag::Double(f64::NAN);
    assert!(
        from_fields(&fields, &items)
            .unwrap_err()
            .starts_with("damage:")
    );
    fields
        .iter_mut()
        .find(|(key, _)| key == "damage")
        .unwrap()
        .1 = Tag::Double(-1.0);
    assert!(
        from_fields(&fields, &items)
            .unwrap_err()
            .starts_with("damage:")
    );
    fields
        .iter_mut()
        .find(|(key, _)| key == "damage")
        .unwrap()
        .1 = Tag::Double(2.0);
    fields.push(("damage".into(), Tag::Double(1.0)));
    assert_eq!(
        from_fields(&fields, &items).unwrap_err(),
        "damage: duplicate field"
    );
    let vanilla = vec![
        ("inGround".into(), Tag::Byte(1)),
        ("pickup".into(), Tag::Byte(0)),
        ("life".into(), Tag::Short(100)),
    ];
    let migrated = from_fields_for(ProjectileKind::Arrow, &vanilla, &items).unwrap();
    assert!(migrated.stuck);
    assert!(
        !migrated.pickup,
        "skeleton arrows never become free ammo after import"
    );
    assert_eq!(migrated.age, 100);
}

#[test]
fn retained_invalid_cloud_and_infinite_motion_are_rejected() {
    let items = items();
    let mut state = ProjectileState::new(
        ProjectileKind::Arrow,
        None,
        stack(&items, "minecraft:arrow"),
    );
    state.damage = f32::INFINITY;
    assert!(validate_state(&state).is_err());
    let mut cloud = super::super::payloads::CloudState::lingering(0);
    cloud.radius = f32::NAN;
    let mut cloud_state = ProjectileState::new(
        ProjectileKind::AreaEffectCloud,
        None,
        stack(&items, "minecraft:lingering_potion"),
    );
    cloud_state.data = ProjectileData::Cloud(cloud);
    assert!(validate_state(&cloud_state).is_err());
    let state = ProjectileState::new(ProjectileKind::Arrow, None, ItemStack::EMPTY);
    let mut fields = to_fields(&state, &items);
    fields.push((
        "Motion".into(),
        Tag::List(mc_nbt::ListTag {
            element_type: mc_nbt::tag_type::DOUBLE,
            elements: vec![
                Tag::Double(f64::INFINITY),
                Tag::Double(0.0),
                Tag::Double(0.0),
            ],
        }),
    ));
    assert!(
        from_fields(&fields, &items)
            .unwrap_err()
            .starts_with("Motion[0]:")
    );
}

fn player_inventory_entry(inventory: Vec<ItemStack>, tick: u64) -> JournalEntry {
    JournalEntry::Player {
        uuid: uuid::Uuid::from_u128(9).to_string(),
        dimension: Default::default(),
        pos: [0.0, 70.0, 0.0],
        yaw: 0.0,
        pitch: 0.0,
        health: 20.0,
        food: 20,
        saturation: 5.0,
        exhaustion: 0.0,
        air: 300,
        experience_level: 0,
        experience_progress: 0.0,
        experience_total: 0,
        game_mode: 0,
        selected: 0,
        inventory,
        carried: ItemStack::EMPTY,
        grid: vec![],
        respawn: None,
        respawn_dimension: Default::default(),
        respawn_forced: false,
        respawn_yaw: 0.0,
        respawn_pitch: 0.0,
        active_effects: Default::default(),
        enchantment_seed: 0,
        warden_warning: Default::default(),
        current_impulse_impact_pos: None,
        burn_ticks: 0,
        opaque_player_fields: Vec::new(),
        statistics: Default::default(),
        advancement_progress: Some(crate::journal::empty_advancement_progress()),
        unlocked_recipes: Some(Default::default()),
        portal: Default::default(),
        fall_flying: false,
        tick,
    }
}

#[test]
fn ammo_consumption_and_pickup_tombstone_share_restart_custody() {
    let items = items();
    let dir = world("custody");
    let owner = uuid::Uuid::from_u128(9).to_string();
    let ammo = stack(&items, "minecraft:trident");
    let initial = replay_batches(
        Replay::default(),
        &[(10, 10, vec![player_inventory_entry(vec![ammo.clone()], 10)])],
    );
    let config = crate::CoreConfig::new(dir.clone(), 42);
    let checkpoint_player = crate::native_checkpoint::tests::roundtrip_player(
        &config,
        &owner,
        &initial.players[&owner],
        &items,
    )
    .unwrap();
    let mut projectile = ProjectileState::new(
        ProjectileKind::Trident,
        Some(uuid::Uuid::from_u128(9)),
        ammo.clone(),
    );
    projectile.pickup = true;
    {
        let mut writer = JournalWriter::open(&dir, 1).unwrap();
        writer
            .append(
                11,
                11,
                &[
                    player_inventory_entry(vec![ItemStack::EMPTY], 11),
                    entry(&projectile, [0.0, 71.0, 1.0], [0.0, 0.0, 50.0]),
                ],
            )
            .unwrap();
        writer.flush().unwrap();
    }
    let mut base = Replay::default();
    base.players
        .insert(owner.clone(), checkpoint_player.clone());
    let recovered = replay_batches(
        base,
        &read_complete_batches_after(&dir.join("journal"), 10).unwrap(),
    );
    assert!(
        recovered.players[&owner]
            .inventory
            .iter()
            .all(ItemStack::is_empty)
    );
    assert_eq!(
        recovered.entities[&7].projectile.as_ref().unwrap().stack,
        ammo
    );
    {
        let mut writer = JournalWriter::open(&dir, 1).unwrap();
        writer
            .append(
                12,
                12,
                &[
                    player_inventory_entry(vec![ammo.clone()], 12),
                    JournalEntry::RemoveEntity { id: 7, tick: 12 },
                ],
            )
            .unwrap();
        writer.flush().unwrap();
    }
    let mut base = Replay::default();
    base.players.insert(owner.clone(), checkpoint_player);
    let recovered = replay_batches(
        base,
        &read_complete_batches_after(&dir.join("journal"), 10).unwrap(),
    );
    assert!(!recovered.entities.contains_key(&7));
    assert_eq!(
        recovered.players[&owner]
            .inventory
            .iter()
            .map(|stack| stack.count)
            .sum::<i32>(),
        1
    );
    assert_eq!(recovered.players[&owner].inventory[0], ammo);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn firework_fuse_and_retained_empty_ammo_roundtrip() {
    let items = items();
    let mut firework = ProjectileState::new(
        ProjectileKind::Firework,
        None,
        stack(&items, "minecraft:firework_rocket"),
    );
    firework.data =
        ProjectileData::Firework(super::super::payloads::FireworkState { lifetime: 43 });
    firework.age = 17;
    firework.shot_at_angle = true;
    let fields = to_fields(&firework, &items);
    assert_eq!(field(&fields, "LifeTime"), Some(&Tag::Int(43)));
    assert_eq!(from_fields(&fields, &items), Ok(firework.clone()));
    firework.attached_to = Some(uuid::Uuid::from_u128(7));
    // FireworkRocketEntity.addAdditionalSaveData does not save its attachment.
    let mut detached = firework.clone();
    detached.attached_to = None;
    assert_eq!(
        from_fields(&to_fields(&firework, &items), &items),
        Ok(detached)
    );
    let mut skeleton = ProjectileState::new(ProjectileKind::Arrow, None, ItemStack::EMPTY);
    skeleton.mob_damage = true;
    skeleton.stuck = true;
    skeleton.age = 44;
    let restored = from_fields(&to_fields(&skeleton, &items), &items).unwrap();
    assert_eq!(restored, skeleton);
    assert!(restored.stack.is_empty());
    assert!(!restored.pickup);
}

#[test]
fn vanilla_cloud_rebases_age_deadlines_and_retained_clock_is_unchanged() {
    let items = items();
    let fields = vec![
        ("Age".into(), Tag::Int(40)),
        ("Duration".into(), Tag::Int(600)),
        ("WaitTime".into(), Tag::Int(10)),
        ("Radius".into(), Tag::Float(2.5)),
    ];
    let mut state = from_fields_for(ProjectileKind::AreaEffectCloud, &fields, &items).unwrap();
    restore_clock(&mut state, 1000);
    let ProjectileData::Cloud(cloud) = &state.data else {
        panic!("cloud")
    };
    assert_eq!(cloud.expires_tick, 1570);
    assert_eq!(cloud.next_tick, 1000);
    let saved = state.clone();
    let mut restored = from_fields(&to_fields(&state, &items), &items).unwrap();
    restore_clock(&mut restored, 1001);
    assert_eq!(
        restored, saved,
        "native retained clocks never restart their duration"
    );
}

#[test]
fn vanilla_acceleration_roundtrips_and_rejects_excessive_power() {
    let items = items();
    let state = ProjectileState::new(ProjectileKind::Fireball, None, ItemStack::EMPTY);
    let mut fields = to_fields(&state, &items);
    assert_eq!(
        field(&fields, "acceleration_power"),
        Some(&Tag::Double(0.1))
    );
    assert_eq!(from_fields(&fields, &items), Ok(state));
    fields
        .iter_mut()
        .find(|(key, _)| key == "acceleration_power")
        .unwrap()
        .1 = Tag::Double(1.01);
    assert!(
        from_fields(&fields, &items)
            .unwrap_err()
            .starts_with("acceleration_power:")
    );
}

#[test]
fn vanilla_infinite_cloud_uses_native_infinite_deadline() {
    let items = items();
    let mut state = from_fields_for(
        ProjectileKind::AreaEffectCloud,
        &[("Duration".into(), Tag::Int(-1))],
        &items,
    )
    .unwrap();
    restore_clock(&mut state, 20);
    let ProjectileData::Cloud(cloud) = &state.data else {
        panic!("cloud")
    };
    assert_eq!(cloud.duration, u32::MAX);
    assert_eq!(cloud.expires_tick, u64::MAX);
    assert_eq!(
        field(&to_fields(&state, &items), "Duration"),
        Some(&Tag::Int(-1))
    );
}

#[test]
fn initial_skeleton_arrow_restore_keeps_stuck_age_and_no_ammo() {
    let items = items();
    let state = ProjectileState::new(ProjectileKind::Arrow, None, ItemStack::EMPTY);
    let entity = image(&state, &items);
    let mut record = replay_image(&entity, state);
    record.projectile = None; // Journal schema before retained projectile state.
    record.age = 733;
    record.appearance.extra.push((
        super::super::ARROW_IN_GROUND_INDEX,
        crate::journal::EntityDataValueImage::Boolean(true),
    ));
    let mut live = live_state();
    crate::core::restore_entity(&mut live, 7, &record).unwrap();
    let slot = live.entities.slot_of(crate::boundary::EntityId(7)).unwrap();
    let restored = live.entities.row(slot).projectile.as_ref().unwrap();
    assert!(restored.stuck);
    assert_eq!(restored.age, 733);
    assert!(restored.stack.is_empty());
    assert!(!restored.pickup);
    record.pos[0] = f64::NAN;
    let error = crate::core::restore_entity(&mut live, 8, &record).unwrap_err();
    assert!(error.to_string().contains("non-finite position"));
    assert!(
        live.entities
            .slot_of(crate::boundary::EntityId(8))
            .is_none()
    );
}

#[test]
fn bad_projectile_numbers_never_publish_a_journal_frame() {
    let items = items();
    let state = ProjectileState::new(
        ProjectileKind::Arrow,
        None,
        stack(&items, "minecraft:arrow"),
    );
    let mut image = serde_json::to_value(entry(&state, [0.0; 3], [0.0; 3])).unwrap();
    image["Entity"]["projectile"]["damage"] = serde_json::json!(-1.0);
    assert!(serde_json::from_value::<JournalEntry>(image).is_err());
    let mut invalid = entry(&state, [0.0; 3], [0.0; 3]);
    let JournalEntry::Entity {
        projectile: Some(projectile),
        ..
    } = &mut invalid
    else {
        unreachable!()
    };
    projectile.damage = f32::NAN;
    let dir = world("bad-journal");
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    assert!(writer.append(11, 11, &[invalid]).is_err());
    assert!(
        read_complete_batches_after(&dir.join("journal"), 0)
            .unwrap()
            .is_empty()
    );
    drop(writer);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn malformed_projectile_fields_return_specific_reasons() {
    let items = items();
    for (field, tag, reason) in [
        ("Owner", Tag::IntArray(vec![1, 2, 3]), "Owner:"),
        ("pickup", Tag::Byte(3), "pickup:"),
        ("inGround", Tag::Int(1), "inGround:"),
        ("PierceLevel", Tag::Int(128), "PierceLevel:"),
        ("life", Tag::Int(-1), "life:"),
        ("damage", Tag::Double(-1.0), "damage:"),
    ] {
        let error =
            from_fields_for(ProjectileKind::Arrow, &[(field.into(), tag)], &items).unwrap_err();
        assert!(error.starts_with(reason), "{field}: {error}");
    }
    let mut entity = image(
        &ProjectileState::new(
            ProjectileKind::Arrow,
            None,
            stack(&items, "minecraft:arrow"),
        ),
        &items,
    );
    entity.motion[2] = f64::NAN;
    assert_eq!(
        from_entity(&entity, &items).unwrap_err(),
        "Motion[2]: non-finite number"
    );
}

fn projectile_anvil_source(root: &std::path::Path) -> std::path::PathBuf {
    let source = root.join("source");
    std::fs::create_dir(&source).unwrap();
    let mut manifest = crate::journal::CheckpointManifest {
        domain_versions: crate::native_checkpoint::domain_versions(),
        world_spawn: Some([0, 80, 0]),
        ..Default::default()
    };
    for dimension in [
        crate::dimensions::Dimension::Nether,
        crate::dimensions::Dimension::End,
    ] {
        manifest.dimensions.insert(
            dimension,
            Box::new(crate::journal::CheckpointManifest {
                domain_versions: crate::native_checkpoint::domain_versions(),
                world_spawn: Some([0, 80, 0]),
                ..Default::default()
            }),
        );
    }
    let mut metadata = crate::level_metadata::MetadataTags::new();
    crate::level_metadata::checkpoint_tags(&mut metadata, 42, &manifest).unwrap();
    for (path, tag) in metadata {
        crate::world_conversion::write_nbt(&source.join(path), &tag).unwrap();
    }
    source
}

fn projectile_import_refusal(root: &std::path::Path, source: &std::path::Path) -> String {
    let destination = root.join("converted");
    let receipt_path = root.join("receipt.json");
    let error = crate::world_conversion::import_anvil(
        &crate::CoreConfig::new(source.to_owned(), 42),
        source,
        &destination,
        &receipt_path,
    )
    .unwrap_err()
    .to_string();
    let receipt: crate::world_conversion::ConversionReceipt =
        serde_json::from_slice(&std::fs::read(receipt_path).unwrap()).unwrap();
    assert_eq!(receipt.status, "FAIL");
    assert_eq!(receipt.error.as_deref(), Some(error.as_str()));
    assert!(
        !destination.exists(),
        "refused import cannot publish a world"
    );
    error
}

fn assert_native_projectile_refusal(
    root: &std::path::Path,
    uuid: uuid::Uuid,
    payload: Tag,
    kind: ProjectileKind,
    expected_uuid: &str,
    reason: &str,
) {
    use crate::native_checkpoint::tests::{fixture_store, read_manifest, write_manifest};
    use mc_world::native::{NativeCheckpoint, NativeEntitySnapshot};

    let config = crate::CoreConfig::new(root.join("runtime"), 42);
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let store = fixture_store(&config).unwrap();
    write_manifest(&config, &read_manifest(&config).unwrap()).unwrap();
    let image = store.read_checkpoint().unwrap();
    let entities = [NativeEntitySnapshot {
        identity: *uuid.as_bytes(),
        dimension: 0,
        position: mc_world::ChunkPos { x: -33, z: 65 },
        generated_slot: None,
        journal_lsn: image.tick,
        payload: Some(payload),
    }];
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: image.tick,
                previous_tick: image.tick,
                chunks: &[],
                entities: &entities,
                players: &[],
                globals: &image.globals,
            },
            &registry.blocks,
            &registry.items,
        )
        .unwrap();
    let before = store.read_checkpoint().unwrap();
    drop(store);

    let error = crate::Core::open(config.clone()).unwrap_err().to_string();
    for detail in [
        config
            .world_dir
            .join("native/state.redb")
            .display()
            .to_string(),
        "minecraft:overworld".into(),
        "chunk (-33, 65)".into(),
        "entity wire-id 7".into(),
        expected_uuid.into(),
        format!("type-id {} ({})", type_id(kind), kind.name()),
        reason.into(),
    ] {
        assert!(error.contains(&detail), "missing {detail:?} in {error}");
    }
    let after = fixture_store(&config).unwrap().read_checkpoint().unwrap();
    assert_eq!(after.tick, before.tick, "refusal cannot advance checkpoint");
    assert_eq!(after.globals.len(), before.globals.len());
    for (key, expected) in &before.globals {
        let actual = &after.globals[key];
        assert_eq!(actual.version, expected.version, "{key}");
        assert_eq!(actual.captured_tick, expected.captured_tick, "{key}");
        assert_eq!(actual.payload, expected.payload, "{key}");
    }
    assert!(before.players.is_empty() && after.players.is_empty());
    assert_eq!(before.entities.len(), 1);
    assert_eq!(after.entities.len(), 1);
    let expected = &before.entities[0];
    let actual = &after.entities[0];
    assert_eq!(actual.identity, expected.identity);
    assert_eq!(actual.dimension, expected.dimension);
    assert_eq!(actual.position, expected.position);
    assert_eq!(actual.generated_slot, expected.generated_slot);
    assert_eq!(actual.journal_lsn, expected.journal_lsn);
    assert_eq!(
        actual.payload, expected.payload,
        "refusal cannot rewrite custody"
    );
}

#[test]
fn malformed_projectile_native_open_and_offline_import_preserve_custody_and_location() {
    let items = items();
    let uuid = uuid::Uuid::from_u128(0xa770);
    let type_id = type_id(ProjectileKind::Arrow);
    for invalid_ledger in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let source = projectile_anvil_source(dir.path());
        let mut projectile = ProjectileState::new(
            ProjectileKind::Arrow,
            None,
            stack(&items, "minecraft:arrow"),
        );
        let mut malformed = image(&projectile, &items);
        malformed.uuid = uuid;
        malformed.pos = [-520.5, 75.0, 1040.5];
        let expected = if invalid_ledger {
            projectile.hit = vec![uuid, uuid];
            let Some(Tag::Compound(runtime)) = malformed
                .extra_fields
                .iter_mut()
                .find(|(key, _)| key == RETAINED)
                .map(|(_, tag)| tag)
            else {
                panic!("runtime");
            };
            runtime
                .iter_mut()
                .find(|(key, _)| key == "state")
                .unwrap()
                .1 = Tag::String(serde_json::to_string(&projectile).unwrap());
            "solaris:ProjectileRuntime.state.hit: duplicate UUID"
        } else {
            let Some(Tag::Compound(item)) = malformed
                .extra_fields
                .iter_mut()
                .find(|(key, _)| key == "item")
                .map(|(_, tag)| tag)
            else {
                panic!("arrow item");
            };
            item.iter_mut().find(|(key, _)| key == "count").unwrap().1 = Tag::Int(2);
            "item.count: expected 1, got 2"
        };
        assert_native_projectile_refusal(
            dir.path(),
            uuid,
            Tag::Compound(mc_data::entity_nbt::entity_to_fields(&items, &malformed).unwrap()),
            ProjectileKind::Arrow,
            &format!("UUID {uuid}"),
            expected,
        );
        let chunks = std::collections::BTreeMap::from([(
            (-33, 65),
            mc_world::anvil::EntityChunk {
                x: -33,
                z: 65,
                entities: vec![
                    mc_data::entity_nbt::VanillaEntity {
                        id: "minecraft:cow".into(),
                        uuid: uuid::Uuid::from_u128(0xa771),
                        wire_id: 6,
                        ..Default::default()
                    },
                    malformed,
                ],
            },
        )]);
        let regions = crate::dimensions::Dimension::Overworld
            .directory(&source)
            .join("entities");
        mc_world::anvil::write_entity_regions(&regions, &items, &chunks, 1).unwrap();
        let path = regions.join("r.-2.2.mca");
        let before = std::fs::read(&path).unwrap();
        let error = projectile_import_refusal(dir.path(), &source);
        for detail in [
            path.display().to_string(),
            "chunk (-33, 65)".into(),
            "entity-index 1".into(),
            format!("UUID {uuid}"),
            format!("type-id {type_id} (minecraft:arrow)"),
            expected.into(),
        ] {
            assert!(error.contains(&detail), "missing {detail:?} in {error}");
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "failed import cannot rewrite custody"
        );
    }
}

#[test]
fn malformed_shared_projectile_native_open_and_offline_import_preserve_raw_identity_and_custody() {
    let items = items();
    let uuid = uuid::Uuid::from_u128(0x5a0b);
    let type_id = type_id(ProjectileKind::Snowball);
    for (bad_field, reason) in [
        ("Item", "Item.id:"),
        ("Motion", "Motion: expected three-double list"),
        ("UUID", "UUID: expected four-int UUID"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let source = projectile_anvil_source(dir.path());
        let projectile = ProjectileState::new(
            ProjectileKind::Snowball,
            None,
            stack(&items, "minecraft:snowball"),
        );
        let mut entity = image(&projectile, &items);
        entity.uuid = uuid;
        entity.pos = [-520.5, 75.0, 1040.5];
        let mut fields = mc_data::entity_nbt::entity_to_fields(&items, &entity).unwrap();
        let value = &mut fields
            .iter_mut()
            .find(|(key, _)| key == bad_field)
            .unwrap()
            .1;
        *value = match bad_field {
            "Item" => Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:no_such_item".into())),
                ("count".into(), Tag::Int(1)),
            ]),
            "Motion" => Tag::String("bad".into()),
            "UUID" => Tag::IntArray(vec![1, 2, 3]),
            _ => unreachable!(),
        };
        let expected_uuid = if bad_field == "UUID" {
            "UUID IntArray([1, 2, 3])".into()
        } else {
            format!("UUID {uuid}")
        };
        assert_native_projectile_refusal(
            dir.path(),
            uuid,
            Tag::Compound(fields.clone()),
            ProjectileKind::Snowball,
            &expected_uuid,
            reason,
        );
        let valid = mc_data::entity_nbt::entity_to_fields(
            &items,
            &mc_data::entity_nbt::VanillaEntity {
                id: "minecraft:cow".into(),
                uuid: uuid::Uuid::from_u128(0x5a0c),
                wire_id: 6,
                ..Default::default()
            },
        )
        .unwrap();
        let tag = Tag::Compound(vec![
            (
                "DataVersion".into(),
                Tag::Int(mc_world::anvil::entity_regions::ENTITY_DATA_VERSION),
            ),
            ("Position".into(), Tag::IntArray(vec![-33, 65])),
            (
                "Entities".into(),
                Tag::List(mc_nbt::ListTag {
                    element_type: mc_nbt::tag_type::COMPOUND,
                    elements: vec![Tag::Compound(valid), Tag::Compound(fields)],
                }),
            ),
        ]);
        let mut bytes = Vec::new();
        mc_nbt::write_named(&mut bytes, "", &tag).unwrap();
        let regions = crate::dimensions::Dimension::Overworld
            .directory(&source)
            .join("entities");
        std::fs::create_dir_all(&regions).unwrap();
        let path = regions.join("r.-2.2.mca");
        mc_world::anvil::region::write_region(
            &path,
            &[mc_world::anvil::region::ChunkPayload {
                local_x: 31,
                local_z: 1,
                timestamp: 1,
                uncompressed_nbt: bytes,
            }],
        )
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        let error = projectile_import_refusal(dir.path(), &source);
        for detail in [
            path.strip_prefix(&source).unwrap().display().to_string(),
            "chunk (-33, 65)".into(),
            "entity-index 1".into(),
            expected_uuid,
            format!("type-id {type_id} (minecraft:snowball)"),
            reason.into(),
        ] {
            assert!(
                error.contains(&detail),
                "{bad_field}: missing {detail:?} in {error}"
            );
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "shared import decode refusal cannot rewrite custody"
        );
    }
}

#[test]
fn crossbow_launch_custody_and_confirmed_kills_round_trip_without_inference() {
    let items = items();
    let mut arrow = ProjectileState::new(
        ProjectileKind::Arrow,
        Some(uuid::Uuid::from_u128(123)),
        stack(&items, "minecraft:arrow"),
    );
    arrow.fired_from_weapon = Some(std::sync::Arc::new(stack(&items, "minecraft:crossbow")));
    arrow
        .crossbow_kills
        .push(serde_json::json!({"type": "minecraft:cow"}));
    let fields = to_fields(&arrow, &items);
    assert_eq!(
        from_fields_for(ProjectileKind::Arrow, &fields, &items).unwrap(),
        arrow
    );

    let vanilla_fields: Vec<_> = fields
        .into_iter()
        .filter(|(key, _)| key != RETAINED)
        .collect();
    let imported = from_fields_for(ProjectileKind::Arrow, &vanilla_fields, &items).unwrap();
    assert!(imported.fired_from_weapon.is_none());
    assert!(imported.crossbow_kills.is_empty());
    arrow.fired_from_weapon = None;
    assert!(validate_state(&arrow).is_err());
    arrow.fired_from_weapon = Some(std::sync::Arc::new(stack(&items, "minecraft:crossbow")));
    arrow
        .crossbow_kills
        .resize(129, serde_json::json!({"type": "minecraft:cow"}));
    assert!(validate_state(&arrow).is_err());
}
