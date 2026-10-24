use super::*;
use crate::boundary::{ChunkPos, ConnectionId, PlayerChange, PlayerInput};
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;
use std::sync::{Arc, LazyLock};
struct Empty(u32);
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(self.0),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}
fn fixture() -> (TickState, EntityId, EntityId) {
    static REG: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    let registry = Arc::clone(&REG);
    let mut world = ChunkWorld::new(
        42,
        Arc::new(Empty(registry.states.air)),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [8, 200, 8],
        mc_world::OVERWORLD_GEOMETRY,
    );
    world.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    let mut state = TickState::new(registry, world, 0, 512);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "swimmer".into(),
            permission_level: 0,
        }],
    );
    let actor = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_pos(slot, [8.5, 200.0, 8.5]);
    let water = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:water").unwrap())
        .unwrap()
        .default
        .0;
    for x in 3..=13 {
        for y in 198..=203 {
            for z in 3..=13 {
                state.world.set_block(x, y, z, water);
            }
        }
    }
    state.world.fold_overlay();
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:dolphin").unwrap())
        .unwrap();
    let dolphin = state.spawn_mob(type_id, [11.5, 200.0, 8.5], 10.0);
    state.players.get_mut(&actor).unwrap().sprinting = true;
    state.tick = 10;
    (state, actor, dolphin)
}
#[test]
fn dolphin_grace_native_effect_refresh_speed_and_expiry() {
    let (mut state, actor, dolphin) = fixture();
    run(&mut state);
    let effects = &state.players[&actor].active_effects;
    assert_eq!(effects.get(EffectId::DolphinsGrace).unwrap().duration, 100);
    assert_eq!(effects.swim_speed_multiplier(), 5.0);
    assert!(state.outbox.values().flatten().any(|event|matches!(event,PlayerChange::MobEffect{effect,..} if effect.id==EffectId::DolphinsGrace)));
    state
        .players
        .get_mut(&actor)
        .unwrap()
        .active_effects
        .entries[0]
        .duration = 40;
    state.tick = 20;
    run(&mut state);
    assert_eq!(
        state.players[&actor]
            .active_effects
            .get(EffectId::DolphinsGrace)
            .unwrap()
            .duration,
        100
    );
    let slot = state.entities.slot_of(dolphin).unwrap();
    state.entities.set_pos(slot, [30.0, 200.0, 8.5]);
    state.tick = 30;
    run(&mut state);
    let player = state.players.get_mut(&actor).unwrap();
    for tick in 0..100 {
        player.active_effects.tick(&mut player.vitals, tick, 20.0);
    }
    assert!(!player.active_effects.has(EffectId::DolphinsGrace));
    assert_eq!(player.active_effects.swim_speed_multiplier(), 1.0);
}
#[test]
fn dolphin_grace_requires_swimming_water_and_live_companion() {
    let (mut state, actor, dolphin) = fixture();
    state.players.get_mut(&actor).unwrap().sprinting = false;
    run(&mut state);
    assert!(
        !state.players[&actor]
            .active_effects
            .has(EffectId::DolphinsGrace)
    );
    state.players.get_mut(&actor).unwrap().sprinting = true;
    let slot = state.entities.slot_of(dolphin).unwrap();
    state.entities.body_mut(slot).alive = false;
    run(&mut state);
    assert!(
        !state.players[&actor]
            .active_effects
            .has(EffectId::DolphinsGrace)
    );
}
#[test]
fn dolphin_fish_claim_is_atomic_and_native_state_roundtrips() {
    let (mut state, actor, dolphin) = fixture();
    let cod = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:cod").unwrap())
        .unwrap();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = mc_data::ItemStack::new(cod, 1);
    let mut intents = Vec::new();
    assert!(interact(
        &mut state,
        actor,
        dolphin,
        Hand::Main,
        &mut intents
    ));
    assert!(interact(
        &mut state,
        actor,
        dolphin,
        Hand::Main,
        &mut intents
    ));
    assert_eq!(state.merge(&mut intents), 1);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    let slot = state.entities.slot_of(dolphin).unwrap();
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    assert!(row.entity_life.dolphin.as_ref().unwrap().got_fish);
    let image = crate::journal::EntityStateImage::from_row(body, row);
    let fields = image.to_fields(&state.registry.blocks);
    assert!(fields.contains(&("GotFish".into(), Tag::Byte(1))));
    let native = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:dolphin".into(),
        extra_fields: fields,
        ..Default::default()
    };
    let recovered =
        crate::journal::EntityStateImage::from_record(&state.registry, &native).unwrap();
    assert_eq!(recovered.entity_life.dolphin, image.entity_life.dolphin);
    assert_eq!(recovered.air_supply, image.air_supply);
    let serde: crate::journal::EntityStateImage =
        serde_json::from_slice(&serde_json::to_vec(&image).unwrap()).unwrap();
    assert_eq!(serde, image);
}
#[test]
fn dolphin_follows_surfaces_and_plays_without_item_duplication() {
    let (mut state, actor, dolphin) = fixture();
    let block_at = |x, y, z| state.world.block_at(x, y, z);
    let pos = [11.5, 200.0, 8.5];
    let velocity = drive(
        pos,
        4800,
        1.2,
        std::iter::once([8.5, 200.0, 8.5]),
        std::iter::empty(),
        &block_at,
        &state.registry,
    )
    .unwrap();
    assert!(velocity[0] < 0.0);
    assert!(
        drive(
            pos,
            100,
            1.2,
            std::iter::empty(),
            std::iter::empty(),
            &block_at,
            &state.registry
        )
        .unwrap()[1]
            > 0.0
    );
    let stone = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    let item = crate::tick::spawn_item_entity(
        &mut state,
        [11.0, 200.0, 8.5],
        mc_data::ItemStack::new(stone, 3),
    );
    let item_slot = state.entities.slot_of(item).unwrap();
    state.entities.body_mut(item_slot).pickup_delay = 0;
    let count = state.entities.len();
    state.tick = 40;
    run(&mut state);
    assert_eq!(state.entities.len(), count);
    assert_eq!(
        state.entities.row(item_slot).item.as_ref().unwrap().count,
        3
    );
    let position = state.entities.pos(item_slot);
    assert!(position[0] > 11.0 && position[1] > 200.0 && position[2] > 8.5);
    let _ = (actor, dolphin);
}
#[test]
fn dolphin_air_depletes_underwater_and_refills_at_surface() {
    assert_eq!(breathe(4800, true), (4799, false));
    assert_eq!(breathe(-19, true), (0, true));
    assert_eq!(breathe(50, false), (4800, false));
    let (mut state, _, dolphin) = fixture();
    let slot = state.entities.slot_of(dolphin).unwrap();
    assert_eq!(state.entities.body(slot).air_supply, 4800);
    state.entities.body_mut(slot).air_supply = 130;
    run_tick(&mut state, &[]);
    assert_eq!(state.entities.body(slot).air_supply, 129);
    assert!(state.entities.vel(slot)[1] > 0.0);
    state.entities.set_pos(slot, [11.5, 203.8, 8.5]);
    run_tick(&mut state, &[]);
    assert_eq!(state.entities.body(slot).air_supply, 4800);
}
#[test]
fn dolphin_corrupt_gotfish_fails_closed_with_exact_field() {
    let record = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:dolphin".into(),
        extra_fields: vec![("GotFish".into(), Tag::Int(1))],
        ..Default::default()
    };
    assert_eq!(
        from_record(&record).unwrap_err().to_string(),
        crate::CoreError::Storage("dolphin GotFish: expected TAG_Byte boolean".into()).to_string()
    );
}

#[test]
fn fed_dolphin_swims_toward_treasure_chest() {
    // DOLPHIN-TREASURE through the dolphin run path: a fed dolphin
    // (GotFish) swims toward a nearby treasure chest.
    let (mut state, _actor, dolphin) = fixture();
    let slot = state.entities.slot_of(dolphin).unwrap();
    let start = state.entities.pos(slot);
    state
        .entities
        .row_mut(slot)
        .entity_life
        .dolphin
        .get_or_insert_with(crate::dolphin::DolphinState::default)
        .got_fish = true;
    // Treasure chest 10 blocks east, same depth.
    let chest = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:chest").unwrap())
        .unwrap()
        .default
        .0;
    let cx = (start[0] as i32) + 10;
    state.world.set_block(cx, start[1] as i32, start[2] as i32, chest);
    state.world.fold_overlay();
    for _ in 0..30 {
        state.tick += 10;
        run(&mut state);
    }
    let end = state.entities.pos(state.entities.slot_of(dolphin).unwrap());
    assert!(
        end[0] > start[0] + 1.0,
        "fed dolphin must swim toward the chest, moved {}",
        end[0] - start[0]
    );
}
