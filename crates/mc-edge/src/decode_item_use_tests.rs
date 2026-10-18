//! Held-item use exercises native packet decoding and the complete core tick.
use super::{DecodeState, decode_play_frame_for};
use mc_core::boundary::{ConnectionId, EntityId, PlayerInput, TickOutput};
use mc_core::config::CoreConfig;
use mc_core::effects::{ActiveEffect, EffectId};
use mc_core::projectiles::ProjectileKind;
use mc_core::registry::CoreRegistry;
use mc_core::tick::{TickState, run_tick};
use mc_core::world::ChunkWorld;
use mc_data::{Identifier, ItemStack};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    Direction, InteractionHand, PlayerActionKind, ServerboundPlayerAction,
    ServerboundSetCarriedItem, ServerboundUseItem, pack_block_pos,
};
use mc_world::{BlockStateId, Chunk, ChunkGenerator, ChunkPos};
use std::sync::{Arc, LazyLock};

const CONNECTION: ConnectionId = ConnectionId(77);

struct Arena {
    air: BlockStateId,
    stone: BlockStateId,
}
impl ChunkGenerator for Arena {
    fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::empty(
            pos,
            self.air,
            Identifier::parse("minecraft:plains").unwrap(),
        );
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, 200, z, self.stone);
            }
        }
        chunk
    }
}

fn fixture() -> (TickState, DecodeState, EntityId) {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> = LazyLock::new(|| {
        let directory =
            std::env::temp_dir().join(format!("ranged-edge-registry-{}", uuid::Uuid::new_v4()));
        let mut config = CoreConfig::new(directory.clone(), 42);
        config.vanilla_data_dir =
            Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
        config.view_distance = 0;
        config.simulation_distance = 0;
        config.checkpoint_every_ticks = u64::MAX;
        let core = mc_core::Core::open(config).unwrap();
        let registry = Arc::clone(core.registry());
        core.shutdown();
        drop(core);
        std::fs::remove_dir_all(directory).unwrap();
        registry
    });
    fixture_with_registry(Arc::clone(&REGISTRY))
}

fn fixture_with_registry(registry: Arc<CoreRegistry>) -> (TickState, DecodeState, EntityId) {
    let generator: Arc<dyn ChunkGenerator> = Arc::new(Arena {
        air: BlockStateId(registry.states.air),
        stone: BlockStateId(registry.states.stone),
    });
    let mut world = ChunkWorld::new(
        42,
        generator,
        &registry,
        BlockStateId(registry.states.air),
        [8, 201, 8],
        mc_world::OVERWORLD_GEOMETRY,
    );
    world.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    let mut state = TickState::new(registry, world, 0, 4096);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: CONNECTION,
            uuid: uuid::Uuid::from_u128(77),
            name: "decoded-archer".into(),
            permission_level: 0,
        }],
    );
    let actor = state.connections.entity_of(CONNECTION).unwrap();
    let slot = entity_slot(&state, actor);
    state.entities.set_pos(slot, [8.5, 201.0, 8.5]);
    state.entities.row_mut(slot).protected_until = 0;
    (state, DecodeState::default(), actor)
}

fn entity_slot(state: &TickState, entity: EntityId) -> mc_core::entities::EntitySlot {
    state
        .entities
        .slots()
        .find(|&slot| state.entities.body(slot).id == entity)
        .unwrap()
}

fn stack(state: &TickState, name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse(name).unwrap())
            .unwrap(),
        count,
    )
}

fn packet<P: Packet>(state: &mut TickState, decoder: &mut DecodeState, packet: P) -> TickOutput {
    let mut body = Vec::new();
    packet.encode(&mut body).unwrap();
    let inputs = decode_play_frame_for(CONNECTION, P::ID, &body, decoder).unwrap();
    assert!(
        !inputs.is_empty(),
        "native ranged packet must reach the core"
    );
    run_tick(state, &inputs).0
}

fn start(state: &mut TickState, decoder: &mut DecodeState, hand: InteractionHand) {
    packet(
        state,
        decoder,
        ServerboundUseItem {
            hand,
            sequence: 1,
            y_rot: 0.0,
            x_rot: 0.0,
        },
    );
}
fn hold(state: &mut TickState, ticks: u64) {
    // The start packet's completed tick counts toward server-side draw time.
    for _ in 1..ticks {
        run_tick(state, &[]);
    }
}
fn release(state: &mut TickState, decoder: &mut DecodeState) -> TickOutput {
    packet(
        state,
        decoder,
        ServerboundPlayerAction {
            action: PlayerActionKind::ReleaseUseItem,
            position: pack_block_pos(0, 0, 0),
            direction: Direction::Down,
            sequence: 2,
        },
    )
}
fn owned(state: &TickState, actor: EntityId, kind: ProjectileKind) -> Vec<EntityId> {
    let owner = state.entities.row(entity_slot(state, actor)).uuid;
    state
        .entities
        .slots()
        .filter_map(|slot| {
            let row = state.entities.row(slot);
            row.projectile
                .as_ref()
                .filter(|projectile| projectile.kind == kind && projectile.owner == Some(owner))
                .map(|_| state.entities.body(slot).id)
        })
        .collect()
}
fn full_power_arrow(state: &TickState, arrow: EntityId) {
    let slot = entity_slot(state, arrow);
    // Core velocity is blocks/second; the edge projects blocks/tick. The
    // release tick has already applied one vanilla 0.99-drag flight step.
    let velocity = state.entities.vel(slot).map(|value| value / 20.0);
    assert!((velocity[2] - 2.97).abs() < 1.0e-6, "{velocity:?}");
    assert!(
        state
            .entities
            .row(slot)
            .projectile
            .as_ref()
            .unwrap()
            .critical
    );
}
fn equip(state: &mut TickState, decoder: &mut DecodeState, actor: EntityId, weapon: &str) {
    let arrows = stack(state, "minecraft:arrow", 4);
    let weapon = stack(state, weapon, 1);
    let player = state.players.get_mut(&actor).unwrap();
    player.inventory.slots[0] = arrows;
    player.inventory.slots[1] = weapon;
    packet(state, decoder, ServerboundSetCarriedItem { slot: 1 });
    assert_eq!(state.players[&actor].inventory.selected, 1);
}

#[test]
fn ranged_decode_bow_full_draw_emits_owned_arrow_and_debits_once() {
    let (mut state, mut decoder, actor) = fixture();
    equip(&mut state, &mut decoder, actor, "minecraft:bow");
    start(&mut state, &mut decoder, InteractionHand::MainHand);
    hold(&mut state, 30);
    let output = release(&mut state, &mut decoder);
    let arrows = owned(&state, actor, ProjectileKind::Arrow);
    assert_eq!(arrows.len(), 1);
    full_power_arrow(&state, arrows[0]);
    assert!(output.changes.entities_spawned.contains(&arrows[0]));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 3);
    assert_eq!(state.players[&actor].inventory.slots[1].damage, Some(1));
    release(&mut state, &mut decoder);
    assert_eq!(owned(&state, actor, ProjectileKind::Arrow).len(), 1);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 3);
}

#[test]
fn ranged_decode_bow_short_draw_keeps_ammo_and_durability() {
    let (mut state, mut decoder, actor) = fixture();
    equip(&mut state, &mut decoder, actor, "minecraft:bow");
    start(&mut state, &mut decoder, InteractionHand::MainHand);
    hold(&mut state, 2);
    release(&mut state, &mut decoder);
    assert!(owned(&state, actor, ProjectileKind::Arrow).is_empty());
    assert_eq!(state.players[&actor].inventory.slots[0].count, 4);
    assert_eq!(
        state.players[&actor].inventory.slots[1].damage.unwrap_or(0),
        0
    );
}

#[test]
fn ranged_decode_crossbow_charges_then_shoots_owned_arrow() {
    let (mut state, mut decoder, actor) = fixture();
    equip(&mut state, &mut decoder, actor, "minecraft:crossbow");
    start(&mut state, &mut decoder, InteractionHand::MainHand);
    hold(&mut state, 25);
    release(&mut state, &mut decoder);
    assert!(owned(&state, actor, ProjectileKind::Arrow).is_empty());
    assert_eq!(
        state.players[&actor].inventory.slots[1]
            .extended()
            .charged_projectiles
            .len(),
        1
    );
    assert_eq!(state.players[&actor].inventory.slots[0].count, 3);
    start(&mut state, &mut decoder, InteractionHand::MainHand);
    assert_eq!(owned(&state, actor, ProjectileKind::Arrow).len(), 1);
    assert!(
        state.players[&actor].inventory.slots[1]
            .extended()
            .charged_projectiles
            .is_empty()
    );
    assert_eq!(state.players[&actor].inventory.slots[0].count, 3);
    assert_eq!(state.players[&actor].inventory.slots[1].damage, Some(1));
}

#[test]
fn ranged_decode_trident_throw_transfers_damaged_stack_to_owned_entity() {
    let (mut state, mut decoder, actor) = fixture();
    equip(&mut state, &mut decoder, actor, "minecraft:trident");
    start(&mut state, &mut decoder, InteractionHand::MainHand);
    hold(&mut state, 10);
    let output = release(&mut state, &mut decoder);
    let tridents = owned(&state, actor, ProjectileKind::Trident);
    assert_eq!(tridents.len(), 1);
    assert!(output.changes.entities_spawned.contains(&tridents[0]));
    assert!(state.players[&actor].inventory.slots[1].is_empty());
    let projectile = state
        .entities
        .row(entity_slot(&state, tridents[0]))
        .projectile
        .as_ref()
        .unwrap();
    assert_eq!(projectile.stack.damage, Some(1));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 4);
}

#[test]
fn ranged_decode_repeated_use_preserves_original_draw_for_all_weapons() {
    for (weapon, ticks, kind) in [
        ("minecraft:bow", 30, ProjectileKind::Arrow),
        ("minecraft:crossbow", 24, ProjectileKind::Arrow),
        ("minecraft:trident", 10, ProjectileKind::Trident),
    ] {
        let (mut state, mut decoder, actor) = fixture();
        equip(&mut state, &mut decoder, actor, weapon);
        start(&mut state, &mut decoder, InteractionHand::MainHand);
        hold(&mut state, ticks);
        packet(
            &mut state,
            &mut decoder,
            ServerboundUseItem {
                hand: InteractionHand::MainHand,
                sequence: 2,
                y_rot: 0.0,
                x_rot: 0.0,
            },
        );
        release(&mut state, &mut decoder);
        if weapon == "minecraft:crossbow" {
            assert_eq!(
                state.players[&actor].inventory.slots[1]
                    .extended()
                    .charged_projectiles
                    .len(),
                1
            );
            assert_eq!(state.players[&actor].inventory.slots[0].count, 3);
            assert!(owned(&state, actor, kind).is_empty());
        } else {
            assert_eq!(owned(&state, actor, kind).len(), 1, "{weapon}");
            if weapon == "minecraft:bow" {
                full_power_arrow(&state, owned(&state, actor, kind)[0]);
            }
        }
    }
}

#[test]
fn ranged_decode_bow_prefers_offhand_ammo_to_inventory_arrows() {
    let (mut state, mut decoder, actor) = fixture();
    equip(&mut state, &mut decoder, actor, "minecraft:bow");
    let offhand = stack(&state, "minecraft:arrow", 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[mc_core::inventory::OFF_HAND] = offhand;
    start(&mut state, &mut decoder, InteractionHand::MainHand);
    hold(&mut state, 25);
    release(&mut state, &mut decoder);
    assert_eq!(owned(&state, actor, ProjectileKind::Arrow).len(), 1);
    assert_eq!(
        state.players[&actor].inventory.slots[mc_core::inventory::OFF_HAND].count,
        1
    );
    assert_eq!(state.players[&actor].inventory.slots[0].count, 4);
    assert_eq!(state.players[&actor].inventory.slots[1].damage, Some(1));
}

#[test]
fn item_use_decode_repeated_golden_apple_use_completes_once_with_effects() {
    let (mut state, mut decoder, actor) = fixture();
    let apples = stack(&state, "minecraft:golden_apple", 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = apples;
    assert_eq!(state.players[&actor].vitals.food, 20);
    assert!(
        state
            .registry
            .item_rules()
            .facts
            .facts_for_stack(
                &state.players[&actor].inventory.slots[0],
                &state.registry.items,
            )
            .unwrap()
            .food
            .as_ref()
            .unwrap()
            .can_always_eat
    );
    for _ in 0..31 {
        start(&mut state, &mut decoder, InteractionHand::MainHand);
    }
    assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
    assert!(
        !state.players[&actor]
            .active_effects
            .has(EffectId::Absorption)
    );
    start(&mut state, &mut decoder, InteractionHand::MainHand);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert!(
        state.players[&actor]
            .active_effects
            .has(EffectId::Absorption)
    );
    assert!(
        state.players[&actor]
            .active_effects
            .has(EffectId::Regeneration)
    );
    assert_eq!(
        state.players[&actor]
            .active_effects
            .get(EffectId::Regeneration)
            .unwrap()
            .amplifier,
        1
    );
    assert_eq!(
        state.players[&actor]
            .active_effects
            .get(EffectId::Absorption)
            .unwrap()
            .amplifier,
        0
    );
    hold(&mut state, 40);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
}

#[test]
fn item_use_decode_repeated_potion_use_completes_with_effect_and_one_bottle() {
    let (mut state, mut decoder, actor) = fixture();
    let mut potion = stack(&state, "minecraft:potion", 1);
    potion.extended_mut().potion = Some(Identifier::parse("minecraft:swiftness").unwrap());
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = potion;
    for _ in 0..31 {
        start(&mut state, &mut decoder, InteractionHand::MainHand);
    }
    assert!(!state.players[&actor].active_effects.has(EffectId::Speed));
    start(&mut state, &mut decoder, InteractionHand::MainHand);
    assert_eq!(
        state.players[&actor].inventory.slots[0],
        stack(&state, "minecraft:glass_bottle", 1)
    );
    assert!(state.players[&actor].active_effects.has(EffectId::Speed));
    hold(&mut state, 40);
    assert_eq!(
        state.players[&actor].inventory.slots[0],
        stack(&state, "minecraft:glass_bottle", 1)
    );
}

#[test]
fn item_use_decode_repeated_milk_use_clears_effects_and_returns_one_bucket() {
    let (mut state, mut decoder, actor) = fixture();
    let milk = stack(&state, "minecraft:milk_bucket", 1);
    let player = state.players.get_mut(&actor).unwrap();
    player.inventory.slots[0] = milk;
    player
        .active_effects
        .add(ActiveEffect::new(EffectId::Poison, 200, 0));
    for _ in 0..31 {
        start(&mut state, &mut decoder, InteractionHand::MainHand);
    }
    assert!(state.players[&actor].active_effects.has(EffectId::Poison));
    start(&mut state, &mut decoder, InteractionHand::MainHand);
    assert!(!state.players[&actor].active_effects.has(EffectId::Poison));
    assert_eq!(
        state.players[&actor].inventory.slots[0],
        stack(&state, "minecraft:bucket", 1)
    );
    hold(&mut state, 40);
    assert_eq!(
        state.players[&actor].inventory.slots[0],
        stack(&state, "minecraft:bucket", 1)
    );
}

#[test]
fn item_use_decode_full_hunger_refuses_bread() {
    let (mut state, mut decoder, actor) = fixture();
    let bread = stack(&state, "minecraft:bread", 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = bread.clone();
    assert_eq!(state.players[&actor].vitals.food, 20);
    for _ in 0..40 {
        start(&mut state, &mut decoder, InteractionHand::MainHand);
    }
    assert_eq!(state.players[&actor].inventory.slots[0], bread);
    assert_eq!(state.players[&actor].vitals.food, 20);
    assert!(state.players[&actor].active_effects.entries.is_empty());
}

#[path = "decode_leash_tests.rs"]
mod leash_tests;

#[path = "decode_spawn_egg_tests.rs"]
mod spawn_egg_tests;

#[path = "decode_item_use_stream_tests.rs"]
mod stream_tests;

#[path = "decode_ranged_wire_tests.rs"]
mod ranged_wire_tests;

#[path = "decode_embedded_projectiles_tests.rs"]
mod embedded_projectiles_tests;

#[path = "decode_item_admission_tests.rs"]
mod item_admission_tests;
