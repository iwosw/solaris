use crate::boundary::{BlockPos, ConnectionId, EntityId, Hand, PlayerInput};
use crate::farming_tests::{Y, block, put, setup};
use crate::tick::{TickState, run_tick};
use mc_data::{Identifier, ItemStack};

const POS: BlockPos = BlockPos { x: 1, y: Y + 1, z: 0 };
fn give(state: &mut TickState, actor: EntityId, name: &str, count: i32) {
    let id = state.registry.items.id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap()).unwrap();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::new(id, count);
}
fn use_on(state: &mut TickState, pos: BlockPos, face: u8, cursor: [f32; 3]) {
    run_tick(state, &[PlayerInput::UseItemOn { connection: ConnectionId(1), hand: Hand::Main, position: pos, face, cursor, sequence: 1 }]);
}
fn install(state: &mut TickState, name: &str, props: &[(&str, &str)]) -> u32 {
    let id = block(state, name, props);
    put(state, POS.x, POS.y, POS.z, id);
    id
}
#[test]
fn interact_parity_axes_preserve_properties_and_wear() {
    let (mut state, actor) = setup();
    give(&mut state, actor, "iron_axe", 1);
    for (source, result, props) in [
        ("oak_log", "stripped_oak_log", &[("axis", "x")][..]),
        ("oxidized_copper", "weathered_copper", &[][..]),
        ("waxed_weathered_copper", "weathered_copper", &[][..]),
    ] {
        install(&mut state, source, props);
        use_on(&mut state, POS, 1, [0.5; 3]);
        assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), Some(block(&state, result, props)), "{source}");
    }
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(3));
}
#[test]
fn interact_parity_sponge_absorbs_adjacent_water() {
    let (mut state, actor) = setup();
    let water = state.registry.states.water_levels[0];
    put(&mut state, 2, Y + 1, 0, water);
    give(&mut state, actor, "sponge", 1);
    use_on(&mut state, BlockPos { y: Y, ..POS }, 1, [0.5, 1.0, 0.5]);
    assert_eq!(state.world.block_at(1, Y + 1, 0), Some(block(&state, "wet_sponge", &[])));
    assert_eq!(state.world.block_at(2, Y + 1, 0), Some(state.registry.states.air));
    assert!(state.players[&actor].inventory.slots[0].is_empty());
}
#[test]
fn interact_parity_dragon_egg_use_teleports() {
    let (mut state, _) = setup();
    let egg = install(&mut state, "dragon_egg", &[]);
    use_on(&mut state, POS, 1, [0.5; 3]);
    assert_ne!(state.world.block_at(POS.x, POS.y, POS.z), Some(egg));
    // Falling is scheduled after teleport; custody is either a block or the
    // native falling entity, never a dropped egg or a duplicate.
    assert!(state.players.values().all(|player| player.inventory.slots.iter().all(|stack| stack.is_empty())));
}
#[test]
fn interact_parity_decorated_pot_stores_one_item() {
    let (mut state, actor) = setup();
    install(&mut state, "decorated_pot", &[("facing", "north"), ("waterlogged", "false"), ("cracked", "false")]);
    give(&mut state, actor, "diamond", 3);
    use_on(&mut state, POS, 2, [0.5; 3]);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
    assert_eq!(state.block_entities[&(POS.x, POS.y, POS.z)].slots[0].count, 1);
}
#[test]
fn interact_parity_chiseled_bookshelf_front_slot() {
    let (mut state, actor) = setup();
    let props = [("facing", "south"), ("slot_0_occupied", "false"), ("slot_1_occupied", "false"), ("slot_2_occupied", "false"), ("slot_3_occupied", "false"), ("slot_4_occupied", "false"), ("slot_5_occupied", "false")];
    install(&mut state, "chiseled_bookshelf", &props);
    give(&mut state, actor, "book", 6);
    use_on(&mut state, POS, 3, [0.1, 0.8, 1.0]);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 5);
    assert_eq!(state.block_entities[&(POS.x, POS.y, POS.z)].slots[0].count, 1);
}
#[test]
fn interact_parity_eternal_fire_magma_and_end_bedrock() {
    let (mut state, _) = setup();
    for (dimension, support) in [(crate::dimensions::Dimension::Overworld, "netherrack"), (crate::dimensions::Dimension::Overworld, "magma_block"), (crate::dimensions::Dimension::End, "bedrock")] {
        state.dimension = dimension;
        let support = block(&state, support, &[]);
        put(&mut state, POS.x, Y, POS.z, support);
        install(&mut state, "fire", &[("age", "15"), ("east", "false"), ("north", "false"), ("south", "false"), ("up", "false"), ("west", "false")]);
        crate::fire::scheduled_tick(&mut state, POS, &mut Vec::new(), &mut Vec::new());
        assert_ne!(state.world.block_at(POS.x, POS.y, POS.z), Some(state.registry.states.air));
    }
}
#[test]
fn interact_parity_painting_places_on_wall() {
    let (mut state, actor) = setup();
    install(&mut state, "stone", &[]);
    give(&mut state, actor, "painting", 1);
    use_on(&mut state, POS, 2, [0.5; 3]);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    let painting = state.registry.entity_type_id("minecraft:painting").unwrap();
    assert_eq!(state.entities.mob_slots(painting).count(), 1);
    let air = state.registry.states.air;
    put(&mut state, POS.x, POS.y, POS.z, air);
    let mut drops = Vec::new();
    crate::painting::blocks_changed(&mut state, &[(POS, air)], &mut drops);
    assert_eq!(state.entities.mob_slots(painting).count(), 0);
    assert_eq!(drops.len(), 1);
    crate::painting::blocks_changed(&mut state, &[(POS, air)], &mut drops);
    assert_eq!(drops.len(), 1);
}

#[test]
fn interact_parity_brushing_completes_once() {
    for (source, result) in [("suspicious_sand", "sand"), ("suspicious_gravel", "gravel")] {
        let (mut state, actor) = setup();
        install(&mut state, source, &[("dusted", "0")]);
        give(&mut state, actor, "brush", 1);
        let slot = state.entities.slot_of(actor).unwrap();
        let [x, y, z] = state.entities.pos(slot);
        let dx = f64::from(POS.x) + 0.5 - x;
        let dz = f64::from(POS.z) + 0.5 - z;
        let dy = f64::from(POS.y) + 0.5 - y - 1.62;
        #[allow(clippy::disallowed_methods)]
        let (yaw, pitch) = {
            let yaw = (-dx).atan2(dz).to_degrees() as f32;
            let pitch = (-dy).atan2(dx.hypot(dz)).to_degrees() as f32;
            (yaw, pitch)
        };
        state.entities.set_rot(slot, yaw, pitch);
        use_on(&mut state, POS, 1, [0.5; 3]);
        for _ in 0..200 { run_tick(&mut state, &[]); }
        assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), Some(block(&state, result, &[])));
        assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(1));
    }
}

#[test]
fn interact_parity_spawn_protection_with_operator() {
    let (mut state, _) = setup();
    run_tick(&mut state, &[PlayerInput::SpawnProtectionOperators { operators: vec![uuid::Uuid::new_v4()] }]);
    let actor = state.connections.entity_of(ConnectionId(1)).unwrap();
    assert!(crate::tick::client_build_refused(&state, actor, &PlayerInput::UseItemOn { connection: ConnectionId(1), hand: Hand::Main, position: POS, face: 1, cursor: [0.5; 3], sequence: 1 }));
    assert!(!crate::spawn_protection::protected(&state, actor, BlockPos { x: 17, ..POS }));
    state.dimension = crate::dimensions::Dimension::Nether;
    assert!(!crate::spawn_protection::protected(&state, actor, POS));
    state.dimension = crate::dimensions::Dimension::Overworld;
    let slot = state.entities.slot_of(actor).unwrap();
    state.spawn_operators.insert(state.entities.row(slot).uuid);
    assert!(!crate::spawn_protection::protected(&state, actor, POS));
    state.spawn_operators.clear();
    assert!(!crate::spawn_protection::protected(&state, actor, POS));
}

#[test]
fn interact_parity_bookshelf_all_slots_extract_and_hydrate() {
    let (mut state, actor) = setup();
    install(&mut state, "chiseled_bookshelf", &[("facing", "south"), ("slot_0_occupied", "false"), ("slot_1_occupied", "false"), ("slot_2_occupied", "false"), ("slot_3_occupied", "false"), ("slot_4_occupied", "false"), ("slot_5_occupied", "false")]);
    give(&mut state, actor, "book", 6);
    for slot in 0..6 {
        use_on(&mut state, POS, 3, [[0.1, 0.5, 0.9][slot % 3], if slot < 3 { 0.8 } else { 0.2 }, 1.0]);
        assert_eq!(crate::special_storage::comparator(&state, POS), Some(slot as u8 + 1));
    }
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    let mut recovered = crate::redstone::custody::BlockEntities::default();
    crate::special_storage::hydrate_chunk(&mut recovered, &state.registry, state.world.chunk(crate::boundary::ChunkPos { x: 0, z: 0 }).unwrap());
    assert_eq!(recovered[&(POS.x,POS.y,POS.z)].slots.iter().map(|stack| stack.count).sum::<i32>(), 6);
    use_on(&mut state, POS, 3, [0.9, 0.2, 1.0]);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert!(state.block_entities[&(POS.x,POS.y,POS.z)].slots[5].is_empty());
}

#[test]
fn interact_parity_sponge_capacity_and_radius() {
    let (mut state, _) = setup();
    let water = state.registry.states.water_levels[0];
    for x in -4..=4 { for y in Y+1..=Y+9 { for z in -4..=4 { put(&mut state,x,y,z,water); } } }
    let origin = BlockPos { x: 0, y: Y+5, z: 0 };
    let dry = block(&state,"sponge",&[]);
    put(&mut state,origin.x,origin.y,origin.z,dry);
    let mut writes = vec![(origin,dry)];
    crate::sponge::after_blocks(&mut state,&mut writes,&mut Vec::new());
    assert_eq!(writes.iter().filter(|(_,id)| *id == state.registry.states.air).count(),64);
    assert_eq!(state.world.block_at(origin.x,origin.y,origin.z),Some(block(&state,"wet_sponge",&[])));

    let (mut state, _) = setup();
    for x in 2..=8 { put(&mut state,x,POS.y,0,water); }
    install(&mut state,"sponge",&[]);
    let mut writes = vec![(POS,dry)];
    crate::sponge::after_blocks(&mut state,&mut writes,&mut Vec::new());
    assert_eq!(state.world.block_at(7,POS.y,0),Some(state.registry.states.air));
    assert_eq!(state.world.block_at(8,POS.y,0),Some(water));
}

#[test]
fn interact_parity_dragon_egg_conserves_block_before_falling() {
    let (mut state, actor) = setup();
    let egg = install(&mut state, "dragon_egg", &[]);
    assert!(crate::vanilla_interact::dragon_egg(&mut state, actor, POS));
    assert_ne!(state.world.block_at(POS.x,POS.y,POS.z),Some(egg));
    let count = (-14..=16).flat_map(|x| (Y-6..=Y+8).flat_map(move |y| (-15..=15).map(move |z| (x,y,z))))
        .filter(|&(x,y,z)| state.world.block_at(x,y,z) == Some(egg)).count();
    assert_eq!(count,1);
}
