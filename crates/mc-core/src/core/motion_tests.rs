use super::*;

#[test]
fn checkpoint_motion_uses_vanilla_tick_units_and_recovers_core_seconds() {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(Arc::clone(&registry), world, 2, 512);
    let cow_type = registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:cow").expect("cow id"))
        .expect("cow type");
    let cow = state.spawn_mob(cow_type, [0.0, 80.0, 0.0], 20.0);
    let slot = state.entities.slot_of(cow).expect("cow slot");
    state.entities.set_vel(slot, [1.0, 0.0, 0.0]);
    let (_, chunks, _) = checkpoint_data_for(&state, 1);
    let record = &chunks[&(0, 0)].entities[0];
    let fields =
        mc_data::entity_nbt::entity_to_fields(&registry.items, record).expect("encode raw NBT");
    let motion = fields
        .iter()
        .find(|(name, _)| name == "Motion")
        .map(|(_, value)| value)
        .expect("Motion tag");
    assert_eq!(
        motion,
        &mc_nbt::Tag::List(mc_nbt::ListTag {
            element_type: mc_nbt::tag_type::DOUBLE,
            elements: vec![
                mc_nbt::Tag::Double(0.05),
                mc_nbt::Tag::Double(0.0),
                mc_nbt::Tag::Double(0.0),
            ],
        })
    );
    let decoded = mc_data::entity_nbt::entity_from_fields(
        &registry.items,
        mc_data::entity_nbt::VanillaEntity::default(),
        &fields,
    )
    .expect("decode raw NBT");
    assert_eq!(
        velocity_from_vanilla_motion(decoded.motion),
        [1.0, 0.0, 0.0]
    );
    assert_eq!(state.entities.vel(slot), [1.0, 0.0, 0.0]);
}

#[test]
fn checkpoint_sheep_preserves_color_and_sheared_until_regrowth() {
    use crate::boundary::{ConnectionId, Hand, PlayerInput};
    use crate::tick::run_tick;

    let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(Arc::clone(&registry), world, 2, 512);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "shearer".to_owned(),
            permission_level: 0,
        }],
    );
    state.world.ensure_blocking(&state.desired_chunks());
    let player = state
        .connections
        .entity_of(ConnectionId(1))
        .expect("player");
    let player_slot = state.entities.slot_of(player).expect("player slot");
    let pos = state.entities.pos(player_slot);
    let sheep_type = registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:sheep").expect("sheep id"))
        .expect("sheep type");
    let sheep = state.spawn_mob(sheep_type, pos, 8.0);
    let sheep_slot = state.entities.slot_of(sheep).expect("sheep slot");
    crate::tick::set_wool(
        Arc::make_mut(&mut state.entities.row_mut(sheep_slot).appearance),
        0x10 | 15,
    );
    let (_, chunks, _) = checkpoint_data_for(&state, 1);
    let record = chunks
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|record| record.id == "minecraft:sheep")
        .expect("checkpoint sheep");
    let fields = mc_data::entity_nbt::entity_to_fields(&registry.items, record).expect("NBT");
    assert!(fields.contains(&("Color".to_owned(), mc_nbt::Tag::Byte(15))));
    assert!(fields.contains(&("Sheared".to_owned(), mc_nbt::Tag::Byte(1))));
    let dir = std::env::temp_dir().join(format!("solaris-sheep-{}", uuid::Uuid::new_v4()));
    mc_world::anvil::entity_regions::write_entity_regions(&dir, &registry.items, &chunks, 1)
        .expect("write checkpoint entities");
    let back = mc_world::anvil::entity_regions::read_entity_regions(&dir, &registry.items)
        .expect("read checkpoint entities");
    std::fs::remove_dir_all(&dir).expect("remove test entities");
    let record = back
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|record| record.id == "minecraft:sheep")
        .expect("recovered sheep");
    assert_eq!(record.sheep_wool, Some(0x10 | 15));
    let replay = crate::journal::ReplayEntity {
        uuid: record.uuid.to_string(),
        type_id: sheep_type,
        pos: record.pos,
        vel: velocity_from_vanilla_motion(record.motion),
        yaw: record.yaw,
        pitch: record.pitch,
        ground: record.on_ground,
        health: record.health,
        alive: true,
        age: record.age as u32,
        breed_cooldown: record.breed_cooldown as u32,
        pickup_delay: record.pickup_delay as u32,
        item: record.item.clone(),
        appearance: appearance_from_vanilla_entity(record),
        active_effects: Default::default(),
        villager: None,
        projectile: None,
        state: Default::default(),
        tick: 1,
    };
    let recovered = EntityId(1000);
    restore_entity(&mut state, recovered.0, &replay).unwrap();
    let slot = state.entities.slot_of(recovered).expect("restored sheep");
    assert_eq!(
        crate::tick::wool_of(&state.entities.row(slot).appearance),
        0x10 | 15
    );
    let shears = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:shears").expect("shears id"))
        .expect("shears");
    state
        .players
        .get_mut(&player)
        .expect("player")
        .inventory
        .slots[0] = mc_data::ItemStack::new(shears, 1);
    let shear = PlayerInput::InteractEntity {
        connection: ConnectionId(1),
        target: recovered,
        hand: Hand::Main,
        location: [0.0; 3],
    };
    let (output, _, _) = run_tick(&mut state, std::slice::from_ref(&shear));
    assert!(
        output.changes.entities_spawned.is_empty(),
        "sheared sheep pays nothing after recovery"
    );
    assert_eq!(state.players[&player].inventory.slots[0].damage, None);
    // Vanilla Sheep.ate clears only the sheared bit when wool regrows.
    let slot = state.entities.slot_of(recovered).expect("restored sheep");
    crate::tick::set_wool(
        Arc::make_mut(&mut state.entities.row_mut(slot).appearance),
        15,
    );
    let (output, _, _) = run_tick(&mut state, &[shear]);
    let black_wool = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:black_wool").expect("wool id"))
        .expect("wool");
    let wool: i32 = output
        .changes
        .entities_spawned
        .iter()
        .filter_map(|id| state.entities.slot_of(*id))
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .filter(|stack| stack.item_id == black_wool)
        .map(|stack| stack.count)
        .sum();
    assert!((1..=3).contains(&wool), "regrown sheep pays colored wool");
    assert_eq!(state.players[&player].inventory.slots[0].damage, Some(1));
}

#[test]
fn checkpoint_orb_restores_value_count_age_and_motion() {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(Arc::clone(&registry), world, 2, 512);
    let mut spawned = Vec::new();
    crate::experience_orbs::award(&mut state, [0.5, 80.0, 0.5], 7, &mut spawned);
    let id = spawned[0];
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).age = 234;
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).experience_orb = Some((7, 4));
    state.entities.set_vel(slot, [1.0, 2.0, 3.0]);
    let (_, chunks, _) = checkpoint_data_for(&state, 100);
    let native = &chunks[&(0, 0)].entities[0];
    let fields = mc_data::entity_nbt::entity_to_fields(&registry.items, native).unwrap();
    let decoded = mc_data::entity_nbt::entity_from_fields(
        &registry.items,
        mc_data::entity_nbt::VanillaEntity::default(),
        &fields,
    )
    .unwrap();
    assert_eq!(decoded.experience_orb, Some((7, 4)));
    assert_eq!(decoded.age, 234);
    let record = crate::journal::ReplayEntity {
        uuid: decoded.uuid.to_string(),
        type_id: registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(decoded.id.clone()).unwrap())
            .unwrap(),
        pos: decoded.pos,
        vel: velocity_from_vanilla_motion(decoded.motion),
        yaw: decoded.yaw,
        pitch: decoded.pitch,
        ground: decoded.on_ground,
        health: decoded.health,
        alive: true,
        age: decoded.age as u32,
        breed_cooldown: 0,
        pickup_delay: 0,
        item: None,
        appearance: appearance_from_vanilla_entity(&decoded),
        active_effects: Default::default(),
        villager: None,
        projectile: None,
        state: Default::default(),
        tick: 100,
    };
    state.remove_entity(slot);
    restore_entity(&mut state, id.0, &record).unwrap();
    let slot = state.entities.slot_of(id).unwrap();
    assert!(matches!(
        state.entities.body(slot).kind,
        EntityKind::ExperienceOrb { .. }
    ));
    assert_eq!(
        state.entities.row(slot).appearance.experience_orb,
        Some((7, 4))
    );
    assert_eq!(state.entities.row(slot).age, 234);
    assert_eq!(state.entities.vel(slot), [1.0, 2.0, 3.0]);
    assert_eq!(
        state.entities.experience_orbs().collect::<Vec<_>>(),
        vec![id]
    );
}
