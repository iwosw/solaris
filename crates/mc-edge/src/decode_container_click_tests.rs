//! Native click decoding, current-custody admission and full-image resync.
use super::{DecodeState, decode_play_frame_for};
use crate::plan::{ViewerState, plan_player_changes};
use mc_core::boundary::{BlockPos, ConnectionId, EntityId, Hand, PlayerChange, PlayerInput};
use mc_core::commands::GameMode;
use mc_core::config::CoreConfig;
use mc_core::inventory::{MenuKind, SlotRef};
use mc_core::player::PlayerState;
use mc_core::registry::CoreRegistry;
use mc_core::tick::{TickState, run_tick};
use mc_core::villager::{VillagerProfession, VillagerTradeOffer};
use mc_core::world::ChunkWorld;
use mc_data::recipes::CookingKind;
use mc_data::{Identifier, ItemStack};
use mc_protocol::frame::Compression;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundContainerSetContent, ContainerInput, HashedStack, ServerboundContainerClick,
};
use mc_world::{BlockStateId, Chunk, ChunkGenerator, ChunkPos};
use std::sync::{Arc, LazyLock};

const CONNECTION: ConnectionId = ConnectionId(77);

fn registry() -> Arc<CoreRegistry> {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> = LazyLock::new(|| {
        let directory =
            std::env::temp_dir().join(format!("click-edge-registry-{}", uuid::Uuid::new_v4()));
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
    Arc::clone(&REGISTRY)
}

fn stack(registry: &CoreRegistry, name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        registry
            .items
            .id_of(&Identifier::parse(name).unwrap())
            .unwrap(),
        count,
    )
}

fn inventory_slot(kind: MenuKind, index: usize) -> i16 {
    (0..kind.slot_count())
        .find(|&slot| kind.slot(slot) == Some(SlotRef::Inventory(index)))
        .unwrap() as i16
}

fn decoded_clicks(window: u8, state_id: i32, slots: [i16; 2]) -> Vec<PlayerInput> {
    let mut decoder = DecodeState::default();
    let mut inputs = Vec::new();
    for slot in slots {
        let packet = ServerboundContainerClick {
            container_id: i32::from(window),
            state_id,
            slot_num: slot,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Empty,
        };
        let mut body = Vec::new();
        packet.encode(&mut body).unwrap();
        let decoded = decode_play_frame_for(
            CONNECTION,
            ServerboundContainerClick::ID,
            &body,
            &mut decoder,
        )
        .unwrap();
        assert!(
            matches!(decoded.as_slice(), [PlayerInput::ContainerClick { state_id: revision, .. }] if *revision == state_id)
        );
        inputs.extend(decoded);
    }
    inputs
}

fn full_images(changes: &[PlayerChange], actor: EntityId) -> Vec<ClientboundContainerSetContent> {
    let mut viewer = ViewerState::new(CONNECTION, actor, [8.5, 201.0, 8.5]);
    plan_player_changes(changes, &mut viewer)
        .into_iter()
        .filter(|packet| packet.id == ClientboundContainerSetContent::ID)
        .map(|packet| {
            let mut framed = bytes::BytesMut::from(&packet.data[..]);
            let frame =
                mc_protocol::frame::try_decode_frame(&mut framed, Compression::Threshold(256))
                    .unwrap()
                    .unwrap();
            ClientboundContainerSetContent::decode(&mut &frame.body[..]).unwrap()
        })
        .collect()
}

#[test]
fn back_to_back_native_clicks_with_old_state_apply_and_resync_every_menu_layout() {
    let registry = registry();
    let rules = registry.item_rules();
    let sticks = stack(&registry, "minecraft:stick", 32);
    let mut kinds = vec![
        MenuKind::Player,
        MenuKind::CraftingTable,
        MenuKind::EnchantingTable,
        MenuKind::Anvil,
        MenuKind::Grindstone,
        MenuKind::Smithing,
        MenuKind::Loom,
        MenuKind::Stonecutter,
        MenuKind::Hopper,
        MenuKind::Dispenser,
        MenuKind::Crafter,
        MenuKind::BrewingStand,
        MenuKind::Merchant,
        MenuKind::Beacon,
        MenuKind::Furnace(CookingKind::Furnace),
        MenuKind::Furnace(CookingKind::Smoker),
        MenuKind::Furnace(CookingKind::BlastFurnace),
    ];
    kinds.extend((1..=6).map(|rows| MenuKind::Chest { rows }));
    assert_eq!(kinds.len(), 23);
    for kind in kinds {
        let mut player = PlayerState::new(CONNECTION, GameMode::Survival);
        player.inventory.slots[0] = sticks.clone();
        let mut container = vec![ItemStack::EMPTY; kind.container_size()];
        player.open_menu(&rules, kind, "Click regression".into(), &container);
        let menu = player.open.as_mut().unwrap_or(&mut player.inventory_menu);
        menu.state_id = 2;
        let window = menu.window;
        let source = inventory_slot(kind, 0);
        let destination = inventory_slot(kind, 1);
        let mut changes = Vec::new();
        for input in decoded_clicks(window, 2, [source, destination]) {
            let PlayerInput::ContainerClick {
                window,
                state_id,
                slot,
                button,
                mode,
                ..
            } = input
            else {
                panic!("native click decoded to another input");
            };
            changes.extend(
                player
                    .click(&rules, window, state_id, slot, button, mode, &mut container)
                    .changes,
            );
        }
        assert!(player.inventory.slots[0].is_empty(), "{kind:?}");
        assert_eq!(player.inventory.slots[1], sticks, "{kind:?}");
        let menu = player.open.as_ref().unwrap_or(&player.inventory_menu);
        assert!(menu.carried.is_empty(), "{kind:?}");
        assert_eq!(menu.state_id, 4, "{kind:?}");
        let images = full_images(&changes, EntityId(77));
        assert_eq!(images.len(), 2, "{kind:?}");
        assert_eq!(images[0].container_id, i32::from(window), "{kind:?}");
        assert_eq!(images[0].state_id, 3, "{kind:?}");
        assert_eq!(images[0].carried_item, sticks, "{kind:?}");
        assert!(images[0].items[source as usize].is_empty(), "{kind:?}");
        assert_eq!(images[1].state_id, 4, "{kind:?}");
        assert!(images[1].carried_item.is_empty(), "{kind:?}");
        assert_eq!(images[1].items[destination as usize], sticks, "{kind:?}");
    }
}

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

fn merchant_fixture() -> (TickState, EntityId, EntityId, mc_core::entities::EntitySlot) {
    let registry = registry();
    let sticks = stack(&registry, "minecraft:stick", 32);
    let emerald = stack(&registry, "minecraft:emerald", 1);
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
            name: "decoded-trader".into(),
            permission_level: 0,
        }],
    );
    let actor = state.connections.entity_of(CONNECTION).unwrap();
    let actor_slot = state
        .entities
        .slots()
        .find(|&slot| state.entities.body(slot).id == actor)
        .unwrap();
    state.entities.set_pos(actor_slot, [8.5, 201.0, 8.5]);
    state.entities.row_mut(actor_slot).protected_until = 0;
    let villager_type = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let merchant = state.spawn_mob(villager_type, [10.5, 201.0, 8.5], 20.0);
    let job = BlockPos {
        x: 11,
        y: 201,
        z: 8,
    };
    let fletching_table = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:fletching_table").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(job.x, job.y, job.z, fletching_table);
    state.world.fold_overlay();
    let merchant_slot = state
        .entities
        .slots()
        .find(|&slot| state.entities.body(slot).id == merchant)
        .unwrap();
    let villager = state
        .entities
        .row_mut(merchant_slot)
        .villager
        .as_mut()
        .unwrap();
    villager.data.profession = VillagerProfession::Fletcher;
    villager.job_site = Some(job);
    villager.offers = vec![VillagerTradeOffer {
        cost_a: sticks.clone(),
        cost_b: None,
        result: emerald.clone(),
        uses: 0,
        max_uses: 16,
        reward_exp: true,
        xp: 1,
        special_price: 0,
        demand: 0,
        price_multiplier: 0.05,
    }];
    let player = state.players.get_mut(&actor).unwrap();
    player.inventory.slots.fill(ItemStack::EMPTY);
    player.inventory.slots[0] = sticks.clone();
    (state, actor, merchant, merchant_slot)
}

#[test]
fn native_stale_payment_click_places_sticks_and_publishes_merchant_result_preview() {
    let (mut state, actor, merchant, merchant_slot) = merchant_fixture();
    let sticks = stack(&state.registry, "minecraft:stick", 32);
    let emerald = stack(&state.registry, "minecraft:emerald", 1);
    run_tick(
        &mut state,
        &[PlayerInput::InteractEntity {
            connection: CONNECTION,
            target: merchant,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
    let menu = state
        .players
        .get_mut(&actor)
        .unwrap()
        .open
        .as_mut()
        .unwrap();
    assert_eq!(menu.kind, MenuKind::Merchant);
    assert!(menu.grid[0].is_empty());
    assert!(menu.result.is_empty());
    menu.state_id = 2;
    let window = menu.window;
    let source = inventory_slot(menu.kind, 0);
    let (_, reliable, _) = run_tick(&mut state, &decoded_clicks(window, 2, [source, 0]));
    let player = &state.players[&actor];
    let menu = player.open.as_ref().unwrap();
    assert!(player.inventory.slots[0].is_empty());
    assert!(menu.carried.is_empty());
    assert_eq!(menu.grid[0], sticks);
    assert!(menu.grid[1].is_empty());
    assert_eq!(menu.result, emerald);
    assert_eq!(menu.state_id, 4);
    assert_eq!(
        state
            .entities
            .row(merchant_slot)
            .villager
            .as_ref()
            .unwrap()
            .offers[0]
            .uses,
        0
    );
    let changes: Vec<_> = reliable
        .into_iter()
        .filter(|(connection, _)| *connection == CONNECTION)
        .flat_map(|(_, changes)| changes)
        .collect();
    let images = full_images(&changes, actor);
    assert_eq!(images.len(), 2);
    assert_eq!(images[0].state_id, 3);
    assert_eq!(images[0].carried_item, sticks);
    assert!(images[0].items[0].is_empty());
    assert!(images[0].items[2].is_empty());
    assert_eq!(images[1].container_id, i32::from(window));
    assert_eq!(images[1].state_id, 4);
    assert!(images[1].carried_item.is_empty());
    assert_eq!(images[1].items[0], sticks);
    assert_eq!(images[1].items[2], emerald);
}

#[test]
fn native_merchant_open_screen_preserves_profession_and_custom_name_components() {
    use mc_protocol::packets::play::ClientboundOpenScreen;

    for name in [
        None,
        Some(r#"Ada "the Fletcher" \ trader"#),
        Some(r#"{"translate":"not.a.profession"}"#),
    ] {
        let (mut state, actor, merchant, slot) = merchant_fixture();
        Arc::make_mut(&mut state.entities.row_mut(slot).appearance).custom_name =
            name.map(Into::into);
        let (_, reliable, _) = run_tick(
            &mut state,
            &[PlayerInput::InteractEntity {
                connection: CONNECTION,
                target: merchant,
                hand: Hand::Main,
                location: [0.0; 3],
            }],
        );
        let changes: Vec<_> = reliable
            .into_iter()
            .flat_map(|(_, changes)| changes)
            .collect();
        let mut viewer = ViewerState::new(CONNECTION, actor, [8.5, 201.0, 8.5]);
        let packets = plan_player_changes(&changes, &mut viewer);
        let mut opens = packets
            .iter()
            .filter(|packet| packet.id == ClientboundOpenScreen::ID);
        let packet = opens.next().expect("merchant interaction opens a screen");
        assert!(opens.next().is_none());
        let mut framed = bytes::BytesMut::from(&packet.data[..]);
        let frame = mc_protocol::frame::try_decode_frame(&mut framed, Compression::Threshold(256))
            .unwrap()
            .unwrap();
        assert!(framed.is_empty());
        let mut body = &frame.body[..];
        let open = ClientboundOpenScreen::decode(&mut body).unwrap();
        assert!(body.is_empty());
        assert_eq!(open.menu_type, 19);
        assert_eq!(
            open.container_id,
            i32::from(state.players[&actor].open.as_ref().unwrap().window)
        );
        let expected = name.map_or_else(
            || {
                mc_nbt::Tag::Compound(vec![(
                    "translate".into(),
                    mc_nbt::Tag::String("entity.minecraft.villager.fletcher".into()),
                )])
            },
            |name| mc_nbt::Tag::String(name.into()),
        );
        let mut title = &open.title_nbt[..];
        assert_eq!(mc_nbt::read_network_value(&mut title).unwrap(), expected);
        assert!(title.is_empty());
    }
}
