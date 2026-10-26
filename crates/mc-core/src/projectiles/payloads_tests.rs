use super::*;
use crate::boundary::{ConnectionId, PlayerInput};
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;

fn fixture() -> (TickState, EntityId) {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
    let terrain = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks));
    let spawn = terrain.locate_safe_spawn().expect("spawn");
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(terrain);
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [spawn.block_x, spawn.surface_y + 1, spawn.block_z],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: Uuid::new_v4(),
            name: "payload-target".into(),
            permission_level: 0,
        }],
    );
    let player = state
        .connections
        .entity_of(ConnectionId(1))
        .expect("player");
    let slot = state.entities.slot_of(player).expect("slot");
    state.entities.set_pos(slot, [10.0, 201.0, 0.0]);
    state.entities.row_mut(slot).protected_until = 0;
    state.outbox.clear();
    (state, player)
}

fn item(state: &TickState, name: &str) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap(),
        1,
    )
}

fn potion(state: &TickState, item_name: &str, potion: &str) -> ItemStack {
    let mut stack = item(state, item_name);
    stack.extended_mut().potion = Some(mc_data::Identifier::parse(potion).unwrap());
    stack
}

fn mob(state: &mut TickState, name: &str, at: [f64; 3]) -> EntityId {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap();
    state.spawn_mob(type_id, at, 20.0)
}

fn launch_record(
    state: &mut TickState,
    record: ProjectileState,
    at: [f64; 3],
) -> (EntityId, ProjectileState) {
    let id = launch(state, record, at, [0.0; 3]).expect("launch");
    let slot = state.entities.slot_of(id).unwrap();
    let record = *state.entities.row_mut(slot).projectile.take().unwrap();
    (id, record)
}

fn damage_to(intents: &[Intent], id: EntityId) -> Vec<(f32, DamageKind)> {
    intents
        .iter()
        .filter_map(|intent| match intent.kind {
            IntentKind::DamageEntity {
                target,
                amount,
                kind,
                ..
            } if target == id => Some((amount, kind)),
            _ => None,
        })
        .collect()
}

fn install_lightning_advancement(
    state: &mut TickState,
    mut tree: mc_data::advancements::AdvancementTree,
) {
    tree.rebuild_trigger_index();
    let mut registry = CoreRegistry::load(None).unwrap();
    registry.advancements = Arc::new(tree);
    state.registry = Arc::new(registry);
    state.advancement_events.clear();
}

fn lightning_advancement(state: &mut TickState) {
    let definition = serde_json::from_value(serde_json::json!({
        "criteria": {
            "accepted": {"trigger": "minecraft:channeled_lightning", "conditions": {}}
        },
        "requirements": [["accepted"]]
    }))
    .unwrap();
    install_lightning_advancement(
        state,
        mc_data::advancements::AdvancementTree {
            definitions: [("test:channeled".into(), definition)]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    );
}

fn bolt_damage(state: &mut TickState, owner: Option<Uuid>, at: [f64; 3]) -> Vec<Intent> {
    let record = ProjectileState::new(ProjectileKind::Lightning, owner, ItemStack::EMPTY);
    let id = launch(state, record, at, [0.0; 3]).unwrap();
    let slot = state.entities.slot_of(id).unwrap();
    let mut record = state.entities.row(slot).projectile.clone().unwrap();
    let mut intents = Vec::new();
    assert!(step(state, &mut intents, id, &mut record));
    intents
}

#[test]
fn channeled_lightning_credits_only_actual_admitted_bolt_victim_to_retained_player_owner() {
    for case in [
        "owned",
        "lethal",
        "natural",
        "mob_owner",
        "missing_owner",
        "rejected",
    ] {
        let (mut state, player) = fixture();
        lightning_advancement(&mut state);
        let victim = mob(&mut state, "minecraft:villager", [0.5, 201.0, 0.5]);
        let slot = state.entities.slot_of(victim).unwrap();
        if case == "lethal" {
            state.entities.row_mut(slot).health = 3.0;
        }
        if case == "rejected" {
            state
                .entities
                .row_mut(slot)
                .active_effects
                .add(ActiveEffect::new(EffectId::FireResistance, 200, 0));
        }
        let owner = match case {
            "natural" => None,
            "missing_owner" => Some(Uuid::new_v4()),
            "mob_owner" => {
                let owner = mob(&mut state, "minecraft:cow", [10.0, 201.0, 0.0]);
                Some(
                    state
                        .entities
                        .row(state.entities.slot_of(owner).unwrap())
                        .uuid,
                )
            }
            _ => Some(
                state
                    .entities
                    .row(state.entities.slot_of(player).unwrap())
                    .uuid,
            ),
        };
        let mut intents = bolt_damage(&mut state, owner, [0.5, 201.0, 0.5]);
        assert_eq!(damage_to(&intents, victim), [(5.0, DamageKind::Lightning)]);
        assert!(
            state.advancement_events.is_empty(),
            "proposal is not victim evidence: {case}"
        );
        state.merge(&mut intents);
        let (deaths, _, _, _) = state.commit_merge_effects();
        let credited = matches!(case, "owned" | "lethal");
        assert_eq!(
            state.advancement_events.len(),
            usize::from(credited),
            "{case}"
        );
        if credited {
            let event = &state.advancement_events[0];
            assert_eq!(event.entity, player);
            assert_eq!(event.trigger, "minecraft:channeled_lightning");
            assert_eq!(event.evidence["victims"].as_array().unwrap().len(), 1);
            assert_eq!(event.evidence["victims"][0]["type"], "minecraft:villager");
            assert_eq!(
                event.evidence["victims"][0]["location"]["position"]["x"],
                0.5
            );
        }
        assert!(deaths.is_empty(), "conversion does not kill the villager");
        let slot = state.entities.slot_of(victim).unwrap();
        assert_eq!(
            entity_name(&state, victim),
            Some(if case == "rejected" {
                "minecraft:villager"
            } else {
                "minecraft:witch"
            })
        );
        assert_eq!(
            state.entities.row(slot).health,
            if case == "rejected" { 20.0 } else { 26.0 }
        );
        crate::advancements::run(&mut state, &mut crate::boundary::ChangeSet::default());
        assert_eq!(
            state.players[&player]
                .advancement_progress
                .get("test:channeled")
                .is_some(),
            credited,
            "{case}"
        );
    }
}

#[test]
fn channeling_trident_launch_and_victim_killed_before_bolt_hits_do_not_count_as_bolt_victims() {
    let (mut state, player) = fixture();
    lightning_advancement(&mut state);
    state.world.ensure_blocking(&state.desired_chunks());
    state.weather.0 = crate::commands::Weather::Thunder;
    let uuid = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    let mut record = ProjectileState::new(
        ProjectileKind::Trident,
        Some(uuid),
        item(&state, "minecraft:trident"),
    );
    record.stack.enchantments.push(mc_data::ItemEnchantment {
        id: mc_data::Identifier::parse("minecraft:channeling").unwrap(),
        level: 1,
    });
    let trident = launch(&mut state, record.clone(), [0.5, 201.0, 0.5], [0.0; 3]).unwrap();
    assert!(
        state.advancement_events.is_empty(),
        "launch is not a lightning hit"
    );
    let victim = mob(&mut state, "minecraft:villager", [0.5, 201.0, 0.5]);
    let slot = state.entities.slot_of(victim).unwrap();
    state.entities.row_mut(slot).health = 3.0;
    let mut intents = Vec::new();
    damage(
        &mut state,
        &mut intents,
        trident,
        victim,
        8.0,
        DamageKind::Projectile,
        Some(player),
        [0.0; 3],
    );
    assert!(channeling(
        &mut state,
        &intents,
        trident,
        &record,
        [0.5, 201.0, 0.5]
    ));
    assert!(
        state.advancement_events.is_empty(),
        "channeling proposal is not a lightning hit"
    );
    state.merge(&mut intents);
    assert_eq!(state.commit_merge_effects().0, [victim]);
    assert!(state.entities.slot_of_any(victim).is_none());
    assert!(
        state.advancement_events.is_empty(),
        "accepted trident spawns a bolt but is not its victim"
    );
    let bolt = state
        .projectiles
        .spawned
        .iter()
        .copied()
        .find(|id| {
            state.entities.slot_of(*id).is_some_and(|slot| {
                state
                    .entities
                    .row(slot)
                    .projectile
                    .as_ref()
                    .is_some_and(|record| record.kind == ProjectileKind::Lightning)
            })
        })
        .unwrap();
    let slot = state.entities.slot_of(bolt).unwrap();
    let mut bolt_record = state.entities.row(slot).projectile.clone().unwrap();
    assert!(step(&mut state, &mut intents, bolt, &mut bolt_record));
    assert!(
        damage_to(&intents, victim).is_empty(),
        "dead trident victim cannot be struck by the later bolt"
    );
    state.merge(&mut intents);
    state.commit_merge_effects();
    crate::advancements::run(&mut state, &mut crate::boundary::ChangeSet::default());
    assert!(
        state.players[&player]
            .advancement_progress
            .get("test:channeled")
            .is_none()
    );
}

#[test]
#[ignore = "requires local vanilla 26.1.2 advancement sidecars"]
fn vanilla_very_very_frightening_requires_owned_accepted_lightning_villager() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/vanilla")
        .canonicalize()
        .expect("local vanilla 26.1.2 advancement sidecars");
    let mut tree = mc_data::advancements::AdvancementTree::load(&root).unwrap();
    let id = "minecraft:adventure/very_very_frightening";
    assert!(tree.definitions.contains_key(id));
    tree.definitions.retain(|key, _| key == id);
    for case in [
        "villager",
        "lethal_villager",
        "cow",
        "natural",
        "rejected",
        "trident_launch",
    ] {
        let (mut state, player) = fixture();
        install_lightning_advancement(&mut state, tree.clone());
        let victim = mob(
            &mut state,
            if case == "cow" {
                "minecraft:cow"
            } else {
                "minecraft:villager"
            },
            [0.5, 201.0, 0.5],
        );
        let slot = state.entities.slot_of(victim).unwrap();
        if case == "lethal_villager" {
            state.entities.row_mut(slot).health = 3.0;
        }
        if case == "rejected" {
            state
                .entities
                .row_mut(slot)
                .active_effects
                .add(ActiveEffect::new(EffectId::FireResistance, 200, 0));
        }
        let owner = (case != "natural").then(|| {
            state
                .entities
                .row(state.entities.slot_of(player).unwrap())
                .uuid
        });
        let mut intents = if case == "trident_launch" {
            let mut record = ProjectileState::new(
                ProjectileKind::Trident,
                owner,
                item(&state, "minecraft:trident"),
            );
            record.stack.enchantments.push(mc_data::ItemEnchantment {
                id: mc_data::Identifier::parse("minecraft:channeling").unwrap(),
                level: 1,
            });
            launch(&mut state, record, [0.5, 201.0, 0.5], [0.0; 3]).unwrap();
            Vec::new()
        } else {
            bolt_damage(&mut state, owner, [0.5, 201.0, 0.5])
        };
        crate::advancements::run(&mut state, &mut crate::boundary::ChangeSet::default());
        assert!(
            state.players[&player]
                .advancement_progress
                .get(id)
                .is_none(),
            "launch alone: {case}"
        );
        state.merge(&mut intents);
        state.commit_merge_effects();
        crate::advancements::run(&mut state, &mut crate::boundary::ChangeSet::default());
        let grants = matches!(case, "villager" | "lethal_villager");
        assert_eq!(
            state.players[&player].advancement_progress[id]["done"]
                .as_bool()
                .unwrap_or(false),
            grants,
            "{case}"
        );
        assert_eq!(
            state.players[&player].advancement_progress[id]["criteria"]
                .get("struck_villager")
                .is_some(),
            grants,
            "{case}"
        );
    }
}

#[test]
fn pearl_retains_owner_uuid_across_wire_identity_change_and_teleports_with_fall_damage() {
    let (mut state, old_owner) = fixture();
    let old_slot = state.entities.slot_of(old_owner).unwrap();
    let uuid = state.entities.row(old_slot).uuid;
    let record = ProjectileState::new(
        ProjectileKind::EnderPearl,
        Some(uuid),
        item(&state, "minecraft:ender_pearl"),
    );
    let encoded = serde_json::to_vec(&record).unwrap();
    // Reconnect through the native lifecycle so the UUID index is updated
    // together with the wire identity, rather than mutating a row behind it.
    run_tick(
        &mut state,
        &[PlayerInput::Leave {
            connection: ConnectionId(1),
        }],
    );
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid,
            name: "payload-target".into(),
            permission_level: 0,
        }],
    );
    let owner = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(owner).unwrap();
    assert_eq!(state.entities.slot_of_uuid(uuid), Some(slot));
    state.entities.set_pos(slot, [0.0, 201.0, 0.0]);
    state.entities.body_mut(slot).burn_ticks = 100;
    state.players.get_mut(&owner).unwrap().fall_distance = 12.0;
    assert_ne!(old_owner, owner);
    let record = serde_json::from_slice(&encoded).unwrap();
    let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    let mut intents = Vec::new();
    assert!(impact(
        &mut state,
        &mut intents,
        id,
        &mut record,
        None,
        [3.0, 205.0, 4.0]
    ));
    assert_eq!(state.entities.pos(slot), [3.0, 205.0, 4.0]);
    assert_eq!(state.players[&owner].fall_distance, 0.0);
    assert_eq!(state.entities.body(slot).burn_ticks, 0);
    assert_eq!(damage_to(&intents, owner), [(5.0, DamageKind::Fall)]);
    assert!(
        state.outbox[&ConnectionId(1)]
            .iter()
            .any(|change| matches!(change,
        PlayerChange::PositionCorrected { position, .. } if *position == [3.0, 205.0, 4.0]))
    );
}

#[test]
fn splash_scales_status_duration_and_emits_player_effect_update() {
    let (mut state, player) = fixture();
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(slot, [2.0, 201.0, 0.0]);
    let record = ProjectileState::new(
        ProjectileKind::Potion,
        None,
        potion(&state, "minecraft:splash_potion", "minecraft:swiftness"),
    );
    let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    assert!(impact(
        &mut state,
        &mut Vec::new(),
        id,
        &mut record,
        None,
        [0.0, 201.0, 0.0]
    ));
    assert_eq!(
        state.players[&player]
            .active_effects
            .get(EffectId::Speed)
            .unwrap()
            .duration,
        1800
    );
    assert!(state.outbox[&ConnectionId(1)].iter().any(|change| matches!(change,
        PlayerChange::MobEffect { effect, .. } if effect.id == EffectId::Speed && effect.duration == 1800)));
    assert!(state.outbox[&ConnectionId(1)].iter().any(|change| matches!(change,
        PlayerChange::ProjectileLevelEvent { event: 2002, data, .. } if *data == potion_color(&record.stack))));
}

#[test]
fn splash_instant_healing_inverts_for_undead_and_keeps_direct_hit_strength() {
    let (mut state, _) = fixture();
    let zombie = mob(&mut state, "minecraft:zombie", [3.0, 201.0, 0.0]);
    let cow = mob(&mut state, "minecraft:cow", [2.0, 201.0, 0.0]);
    let cow_slot = state.entities.slot_of(cow).unwrap();
    state.entities.row_mut(cow_slot).health = 6.0;
    let record = ProjectileState::new(
        ProjectileKind::Potion,
        None,
        potion(&state, "minecraft:splash_potion", "minecraft:healing"),
    );
    let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    let mut intents = Vec::new();
    assert!(impact(
        &mut state,
        &mut intents,
        id,
        &mut record,
        Some(zombie),
        [0.0, 201.0, 0.0]
    ));
    assert_eq!(
        damage_to(&intents, zombie),
        [(6.0, DamageKind::IndirectMagic)]
    );
    assert_eq!(state.entities.row(cow_slot).health, 8.0);
    assert!(
        state
            .entities
            .row(cow_slot)
            .active_effects
            .entries
            .is_empty()
    );
}

#[test]
fn lingering_cloud_serializes_timers_and_reapplication_identity_not_entity_id() {
    let (mut state, _) = fixture();
    state.tick = 100;
    let target = mob(&mut state, "minecraft:cow", [1.0, 201.0, 0.0]);
    let target_slot = state.entities.slot_of(target).unwrap();
    let target_uuid = state.entities.row(target_slot).uuid;
    let record = ProjectileState::new(
        ProjectileKind::LingeringPotion,
        None,
        potion(&state, "minecraft:lingering_potion", "minecraft:harming"),
    );
    let (bottle, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    assert!(impact(
        &mut state,
        &mut Vec::new(),
        bottle,
        &mut record,
        Some(target),
        [0.0, 201.0, 0.0]
    ));
    remove(&mut state, bottle);
    let cloud_id = state
        .projectiles
        .spawned
        .iter()
        .copied()
        .find(|id| {
            state.entities.slot_of(*id).is_some_and(|slot| {
                state
                    .entities
                    .row(slot)
                    .projectile
                    .as_ref()
                    .is_some_and(|record| record.kind == ProjectileKind::AreaEffectCloud)
            })
        })
        .unwrap();
    let cloud_slot = state.entities.slot_of(cloud_id).unwrap();
    let mut cloud = *state
        .entities
        .row_mut(cloud_slot)
        .projectile
        .take()
        .unwrap();
    let mut intents = Vec::new();
    cloud.age = 9;
    state.tick = 109;
    assert!(step(&mut state, &mut intents, cloud_id, &mut cloud));
    assert!(intents.is_empty());
    cloud.age = 10;
    state.tick = 110;
    assert!(step(&mut state, &mut intents, cloud_id, &mut cloud));
    assert_eq!(
        damage_to(&intents, target),
        [(3.0, DamageKind::IndirectMagic)]
    );
    let ProjectileData::Cloud(data) = &cloud.data else {
        panic!("cloud");
    };
    assert_eq!(data.affected_until[&target_uuid], 210);
    assert_eq!(data.next_tick, 115);
    assert_eq!(data.expires_tick, 710);
    assert!((data.radius - 2.475).abs() < 0.00001);
    assert!(!data.waiting);
    let encoded = serde_json::to_vec(&cloud).unwrap();
    let restored: ProjectileState = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(restored, cloud);
    cloud = restored;
    // Replay can assign another wire id to the same persistent victim UUID.
    let target_slot = state.entities.slot_of(target).unwrap();
    state.remove_entity(target_slot);
    let replacement = mob(&mut state, "minecraft:cow", [1.0, 201.0, 0.0]);
    let replacement_slot = state.entities.slot_of(replacement).unwrap();
    state.entities.set_uuid(replacement_slot, target_uuid);
    assert_ne!(replacement, target);
    intents.clear();
    state.tick = 115;
    cloud.age = 15;
    step(&mut state, &mut intents, cloud_id, &mut cloud);
    assert!(
        intents.is_empty(),
        "UUID reapplication deadline survives replay"
    );
    state.tick = 210;
    cloud.age = 110;
    step(&mut state, &mut intents, cloud_id, &mut cloud);
    assert_eq!(
        damage_to(&intents, replacement),
        [(3.0, DamageKind::IndirectMagic)]
    );
    state.tick = 710;
    cloud.age = 610;
    step(&mut state, &mut Vec::new(), cloud_id, &mut cloud);
    assert!(state.entities.slot_of(cloud_id).is_none());
}

#[test]
fn shulker_fireball_skull_wind_and_spit_deliver_native_special_payloads() {
    let (mut state, _) = fixture();
    for (kind, expected) in [
        (ProjectileKind::SmallFireball, 5.0),
        (ProjectileKind::Fireball, 6.0),
        (ProjectileKind::WitherSkull, 8.0),
        (ProjectileKind::ShulkerBullet, 4.0),
        (ProjectileKind::WindCharge, 1.0),
        (ProjectileKind::BreezeWindCharge, 1.0),
        (ProjectileKind::LlamaSpit, 1.0),
    ] {
        let target = mob(&mut state, "minecraft:cow", [0.0, 201.0, 0.0]);
        let record = ProjectileState::new(kind, None, ItemStack::EMPTY);
        let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
        let mut intents = Vec::new();
        assert!(impact(
            &mut state,
            &mut intents,
            id,
            &mut record,
            Some(target),
            [0.0, 201.0, 0.0]
        ));
        assert_eq!(
            damage_to(&intents, target),
            [(
                expected,
                if matches!(
                    kind,
                    ProjectileKind::SmallFireball | ProjectileKind::Fireball
                ) {
                    DamageKind::Fireball
                } else {
                    DamageKind::Projectile
                }
            )],
            "{kind:?}"
        );
        let slot = state.entities.slot_of(target).unwrap();
        if kind == ProjectileKind::SmallFireball {
            assert_eq!(
                state.entities.body(slot).burn_ticks,
                0,
                "ignition waits for accepted hit"
            );
            assert!(intents.iter().any(|intent| matches!(
                intent.kind,
                IntentKind::DamageEntity {
                    source: Some(crate::intent::DamageSource::Projectile { flame: true, .. }),
                    ..
                }
            )));
        }
        if kind == ProjectileKind::ShulkerBullet {
            assert!(
                !state
                    .entities
                    .row(slot)
                    .active_effects
                    .has(EffectId::Levitation)
            );
            let source = intents
                .iter()
                .find_map(|intent| match intent.kind {
                    IntentKind::DamageEntity { source, .. } => source,
                    _ => None,
                })
                .unwrap();
            assert!(accepted_hit(&mut state, target, source, &mut 5.0).is_empty());
            assert_eq!(
                state
                    .entities
                    .row(slot)
                    .active_effects
                    .get(EffectId::Levitation)
                    .unwrap()
                    .duration,
                200
            );
        }
        if matches!(kind, ProjectileKind::Fireball | ProjectileKind::WitherSkull) {
            assert!(
                intents
                    .iter()
                    .any(|intent| matches!(intent.kind, IntentKind::Explode { power: 1.0, .. }))
            );
        }
        if matches!(
            kind,
            ProjectileKind::WindCharge | ProjectileKind::BreezeWindCharge
        ) {
            assert!(
                intents
                    .iter()
                    .all(|intent| !matches!(intent.kind, IntentKind::Explode { .. }))
            );
            assert_eq!(state.hostile.explosions.last().unwrap().blocks, 0);
        }
        remove(&mut state, id);
        let slot = state.entities.slot_of(target).unwrap();
        state.remove_entity(slot);
    }
}

#[test]
fn small_fireball_does_not_ignite_fire_resistant_target() {
    let (mut state, player) = fixture();
    give_effect(
        &mut state,
        player,
        ActiveEffect::new(EffectId::FireResistance, 600, 0),
    );
    let record = ProjectileState::new(ProjectileKind::SmallFireball, None, ItemStack::EMPTY);
    let (id, mut record) = launch_record(&mut state, record, [10.0, 201.0, 0.0]);
    let mut intents = Vec::new();
    impact(
        &mut state,
        &mut intents,
        id,
        &mut record,
        Some(player),
        [10.0, 201.0, 0.0],
    );
    assert_eq!(damage_to(&intents, player), [(0.0, DamageKind::Fireball)]);
    assert_eq!(
        state
            .entities
            .body(state.entities.slot_of(player).unwrap())
            .burn_ticks,
        0
    );
}

#[test]
fn spectral_and_tipped_arrow_payloads_use_native_effects() {
    let (mut state, player) = fixture();
    let spectral = ProjectileState::new(
        ProjectileKind::SpectralArrow,
        None,
        item(&state, "minecraft:spectral_arrow"),
    );
    arrow_effects(
        &mut state,
        &mut Vec::new(),
        EntityId(1000),
        &spectral,
        player,
    );
    assert!(!state.players[&player].active_effects.has(EffectId::Glowing));
    let source = crate::intent::DamageSource::Projectile {
        projectile: EntityId(1000),
        direct_type_id: 0,
        owner: player,
        velocity: [1.0, 0.0],
        punch: 0,
        flame: false,
    };
    assert!(accepted_hit(&mut state, player, source, &mut 5.0).is_empty());
    assert_eq!(
        state.players[&player]
            .active_effects
            .get(EffectId::Glowing)
            .unwrap()
            .duration,
        200
    );
    let tipped = ProjectileState::new(
        ProjectileKind::Arrow,
        None,
        potion(&state, "minecraft:tipped_arrow", "minecraft:poison"),
    );
    arrow_effects(&mut state, &mut Vec::new(), EntityId(1001), &tipped, player);
    assert!(!state.players[&player].active_effects.has(EffectId::Poison));
    let source = crate::intent::DamageSource::Projectile {
        projectile: EntityId(1001),
        direct_type_id: 0,
        owner: player,
        velocity: [1.0, 0.0],
        punch: 0,
        flame: false,
    };
    assert!(accepted_hit(&mut state, player, source, &mut 5.0).is_empty());
    assert_eq!(
        state.players[&player]
            .active_effects
            .get(EffectId::Poison)
            .unwrap()
            .duration,
        112
    );
}

#[test]
fn firework_retains_lifetime_and_uses_26_1_item_attachment_angle_metadata() {
    let (mut state, _) = fixture();
    let mut record = ProjectileState::new(
        ProjectileKind::Firework,
        None,
        item(&state, "minecraft:firework_rocket"),
    );
    record.shot_at_angle = true;
    let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    let ProjectileData::Firework(data) = &record.data else {
        panic!("retained firework");
    };
    let lifetime = data.lifetime;
    assert!((11..=22).contains(&lifetime));
    let copy: ProjectileState =
        serde_json::from_slice(&serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(copy, record);
    let appearance = &state
        .entities
        .row(state.entities.slot_of(id).unwrap())
        .appearance;
    assert!(
        appearance
            .extra
            .contains(&(9, EntityDataValue::OptionalUnsignedInt(None)))
    );
    assert!(
        appearance
            .extra
            .contains(&(10, EntityDataValue::Boolean(true)))
    );
    record.age = lifetime;
    assert!(step(&mut state, &mut Vec::new(), id, &mut record));
    assert!(state.entities.slot_of(id).is_none());
    assert!(
        state.outbox[&ConnectionId(1)]
            .iter()
            .any(|change| matches!(change,
        PlayerChange::ProjectileStatus { entity, event: 17, .. } if *entity == id))
    );
}

#[test]
fn attached_firework_reports_owner_id_and_ignores_owner_collision() {
    let (mut state, player) = fixture();
    let owner_uuid = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    let position = state.entities.pos(state.entities.slot_of(player).unwrap());
    let mut record = ProjectileState::new(
        ProjectileKind::Firework,
        Some(owner_uuid),
        item(&state, "minecraft:firework_rocket"),
    );
    record.attached_to = Some(owner_uuid);
    let (id, mut record) = launch_record(&mut state, record, position);
    let player_slot = state.entities.slot_of(player).unwrap();
    let expected = Some(state.entities.body(player_slot).id.0 as u32);
    state
        .entities
        .row_mut(state.entities.slot_of(id).unwrap())
        .projectile = Some(Box::new(record.clone()));
    initialize(&mut state, id);
    let appearance = &state
        .entities
        .row(state.entities.slot_of(id).unwrap())
        .appearance;
    assert!(
        appearance
            .extra
            .contains(&(9, EntityDataValue::OptionalUnsignedInt(expected)))
    );
    // A replayed appearance can contain an old connection's wire id.
    // Rebuild it from the runtime UUID before publishing.
    set_metadata(
        &mut state,
        id,
        9,
        EntityDataValue::OptionalUnsignedInt(Some(999_999)),
    );
    let slot = state.entities.slot_of(id).unwrap();
    super::super::initialize_appearance(&mut state, slot, &record);
    assert!(
        state
            .entities
            .row(slot)
            .appearance
            .extra
            .contains(&(9, EntityDataValue::OptionalUnsignedInt(expected)))
    );
    record = *state
        .entities
        .row_mut(state.entities.slot_of(id).unwrap())
        .projectile
        .take()
        .unwrap();
    assert!(!impact(
        &mut state,
        &mut Vec::new(),
        id,
        &mut record,
        Some(player),
        position
    ));
    let restored: ProjectileState =
        serde_json::from_value(serde_json::to_value(&record).unwrap()).unwrap();
    assert!(
        restored.attached_to.is_none(),
        "vanilla does not save rocket attachments"
    );
    super::super::initialize_appearance(&mut state, slot, &restored);
    assert!(
        state
            .entities
            .row(slot)
            .appearance
            .extra
            .contains(&(9, EntityDataValue::OptionalUnsignedInt(None))),
        "replay must clear the old wire entity id"
    );
}

#[test]
fn experience_bottle_awards_native_orb_denominations_and_water_tint_event() {
    let (mut state, _) = fixture();
    let record = ProjectileState::new(
        ProjectileKind::ExperienceBottle,
        None,
        item(&state, "minecraft:experience_bottle"),
    );
    let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    assert!(impact(
        &mut state,
        &mut Vec::new(),
        id,
        &mut record,
        None,
        [0.0, 201.0, 0.0]
    ));
    let amount: i32 = state
        .entities
        .experience_orbs()
        .map(|id| {
            let slot = state.entities.slot_of(id).unwrap();
            let (value, count) = state.entities.row(slot).appearance.experience_orb.unwrap();
            value * count
        })
        .sum();
    assert!((3..=11).contains(&amount));
    assert!(state.outbox[&ConnectionId(1)].iter().any(|change| matches!(
        change,
        PlayerChange::ProjectileLevelEvent {
            event: 2002,
            data: -13_083_194,
            ..
        }
    )));
}

#[test]
fn egg_hatches_native_baby_chicken_with_breed_cooldown() {
    let (mut state, _) = fixture();
    let record = ProjectileState::new(ProjectileKind::Egg, None, item(&state, "minecraft:egg"));
    let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    let seed = (1_u64..1000)
        .find(|seed| random(*seed ^ state.tick).is_multiple_of(8))
        .unwrap();
    let slot = state.entities.slot_of(id).unwrap();
    state
        .entities
        .set_uuid(slot, Uuid::from_u128(u128::from(seed)));
    assert!(impact(
        &mut state,
        &mut Vec::new(),
        id,
        &mut record,
        None,
        [0.0, 201.0, 0.0]
    ));
    let chickens: Vec<_> = state
        .projectiles
        .spawned
        .iter()
        .filter_map(|id| {
            let slot = state.entities.slot_of(*id)?;
            (entity_name(&state, *id) == Some("minecraft:chicken")).then_some(slot)
        })
        .collect();
    assert!(matches!(chickens.len(), 1 | 4));
    for slot in chickens {
        let row = state.entities.row(slot);
        assert!(row.appearance.baby);
        assert_eq!(row.entity_life.child_ticks, 24_000);
        let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
            panic!("chicken mob");
        };
        assert_eq!(
            row.breed_cooldown,
            state
                .registry
                .blueprint(type_id)
                .unwrap()
                .params
                .breed_cooldown
        );
    }
}

#[test]
fn channeling_requires_enchantment_thunder_open_sky_and_accepted_hit() {
    let (mut state, player) = fixture();
    let wanted = state.desired_chunks();
    state.world.ensure_blocking(&wanted);
    let uuid = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    let target = mob(&mut state, "minecraft:cow", [0.5, 201.0, 0.5]);
    let mut record = ProjectileState::new(
        ProjectileKind::Trident,
        Some(uuid),
        item(&state, "minecraft:trident"),
    );
    let mut intents = Vec::new();
    damage(
        &mut state,
        &mut intents,
        EntityId(1000),
        target,
        8.0,
        DamageKind::Projectile,
        Some(player),
        [1.0, 0.0, 0.0],
    );
    state.weather.0 = crate::commands::Weather::Thunder;
    assert!(!channeling(
        &mut state,
        &intents,
        EntityId(1000),
        &record,
        [0.5, 201.0, 0.5]
    ));
    record.stack.enchantments.push(mc_data::ItemEnchantment {
        id: mc_data::Identifier::parse("minecraft:channeling").unwrap(),
        level: 1,
    });
    state.weather.0 = crate::commands::Weather::Clear;
    assert!(!channeling(
        &mut state,
        &intents,
        EntityId(1000),
        &record,
        [0.5, 201.0, 0.5]
    ));
    state.weather.0 = crate::commands::Weather::Thunder;
    state
        .world
        .set_block(0, 210, 0, state.registry.states.stone);
    assert!(!channeling(
        &mut state,
        &intents,
        EntityId(1000),
        &record,
        [0.5, 201.0, 0.5]
    ));
    state.world.set_block(0, 210, 0, state.registry.states.air);
    assert!(channeling(
        &mut state,
        &intents,
        EntityId(1000),
        &record,
        [0.5, 201.0, 0.5]
    ));
    assert!(
        state
            .entities
            .slots()
            .all(|slot| entity_name(&state, state.entities.body(slot).id)
                != Some("minecraft:lightning_bolt"))
    );
    let source = intents
        .iter()
        .find_map(|intent| match intent.kind {
            IntentKind::DamageEntity { source, .. } => source,
            _ => None,
        })
        .unwrap();
    assert!(accepted_hit(&mut state, target, source, &mut 5.0).is_empty());
    let lightning = state
        .projectiles
        .spawned
        .iter()
        .copied()
        .find(|id| entity_name(&state, *id) == Some("minecraft:lightning_bolt"))
        .unwrap();
    let slot = state.entities.slot_of(lightning).unwrap();
    assert_eq!(
        state.entities.row(slot).projectile.as_ref().unwrap().owner,
        Some(uuid)
    );
}

#[test]
fn refused_arrow_effects_do_not_mutate_victim_and_accepted_instant_payload_returns_magic_piece() {
    let (mut state, player) = fixture();
    let poison = ProjectileState::new(
        ProjectileKind::Arrow,
        None,
        potion(&state, "minecraft:tipped_arrow", "minecraft:poison"),
    );
    arrow_effects(&mut state, &mut Vec::new(), EntityId(1000), &poison, player);
    // A refused direct hit never invokes accepted_hit: no poison or player packet.
    assert!(!state.players[&player].active_effects.has(EffectId::Poison));
    assert!(state.outbox.get(&ConnectionId(1)).is_none_or(Vec::is_empty));
    state.projectiles.after_hit.clear();
    let harming = ProjectileState::new(
        ProjectileKind::Arrow,
        None,
        potion(&state, "minecraft:tipped_arrow", "minecraft:harming"),
    );
    arrow_effects(
        &mut state,
        &mut Vec::new(),
        EntityId(1001),
        &harming,
        player,
    );
    let source = crate::intent::DamageSource::Projectile {
        projectile: EntityId(1001),
        direct_type_id: 0,
        owner: player,
        velocity: [1.0, 0.0],
        punch: 0,
        flame: false,
    };
    let extras = accepted_hit(&mut state, player, source, &mut 5.0);
    assert_eq!(extras.len(), 1);
    assert_eq!(extras[0].amount, 6.0);
    assert_eq!(extras[0].kind, DamageKind::Magic);
    assert!(extras[0].source.is_none());
    assert!(state.players[&player].active_effects.entries.is_empty());
}

#[test]
fn fireball_and_lightning_preserve_fire_and_armor_mitigation_categories() {
    let (state, _) = fixture();
    let armor = [
        item(&state, "minecraft:diamond_boots"),
        item(&state, "minecraft:diamond_leggings"),
        item(&state, "minecraft:diamond_chestplate"),
        item(&state, "minecraft:diamond_helmet"),
    ];
    let rules = state.registry.item_rules();
    for kind in [DamageKind::Fireball, DamageKind::Lightning] {
        assert!(crate::survival::reduce_damage(&rules, &armor, 6.0, kind) < 6.0);
        let mut effects = crate::effects::ActiveEffects::default();
        effects.add(ActiveEffect::new(EffectId::FireResistance, 200, 0));
        assert_eq!(effects.reduce_damage(kind, 6.0), 0.0);
    }
    assert_eq!(
        crate::survival::reduce_damage(&rules, &armor, 6.0, DamageKind::Generic),
        6.0
    );
}

#[test]
fn default_cloud_has_vanilla_infinite_duration_and_default_particle() {
    let (mut state, _) = fixture();
    let record = ProjectileState::new(ProjectileKind::AreaEffectCloud, None, ItemStack::EMPTY);
    let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    let ProjectileData::Cloud(cloud) = &record.data else {
        panic!("retained cloud");
    };
    assert_eq!(
        (
            cloud.radius,
            cloud.duration,
            cloud.wait_time,
            cloud.reapplication_delay
        ),
        (3.0, u32::MAX, 20, 20)
    );
    let appearance = &state
        .entities
        .row(state.entities.slot_of(id).unwrap())
        .appearance;
    assert!(
        appearance
            .extra
            .contains(&(10, EntityDataValue::EntityEffectParticle(-1)))
    );
    record.age = 1201;
    state.tick = 1201;
    assert!(step(&mut state, &mut Vec::new(), id, &mut record));
    assert!(
        state.entities.slot_of(id).is_some(),
        "infinite vanilla cloud survives ordinary flight expiry"
    );
}

#[test]
fn tipped_instant_harming_enforces_pvp_but_self_hits_and_healing_remain_admitted() {
    let (mut state, target) = fixture();
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(2),
            uuid: Uuid::new_v4(),
            name: "arrow-owner".into(),
            permission_level: 0,
        }],
    );
    let owner = state.connections.entity_of(ConnectionId(2)).unwrap();
    let harming = ProjectileState::new(
        ProjectileKind::Arrow,
        None,
        potion(&state, "minecraft:tipped_arrow", "minecraft:harming"),
    );
    for (pvp, shooter, expected) in [(false, owner, 0), (true, owner, 1), (false, target, 1)] {
        state.gamerules.insert("pvp".into(), pvp.to_string());
        arrow_effects(
            &mut state,
            &mut Vec::new(),
            EntityId(2000),
            &harming,
            target,
        );
        let extras = accepted_hit(
            &mut state,
            target,
            crate::intent::DamageSource::Projectile {
                projectile: EntityId(2000),
                direct_type_id: 0,
                owner: shooter,
                velocity: [1.0, 0.0],
                punch: 0,
                flame: false,
            },
            &mut 5.0,
        );
        assert_eq!(extras.len(), expected, "pvp={pvp} owner={shooter:?}");
        assert!(
            extras
                .iter()
                .all(|piece| piece.kind == DamageKind::Magic && piece.amount == 6.0)
        );
    }
    state.players.get_mut(&target).unwrap().vitals.health = 10.0;
    let slot = state.entities.slot_of(target).unwrap();
    state.entities.row_mut(slot).health = 10.0;
    let healing = ProjectileState::new(
        ProjectileKind::Arrow,
        None,
        potion(&state, "minecraft:tipped_arrow", "minecraft:healing"),
    );
    arrow_effects(
        &mut state,
        &mut Vec::new(),
        EntityId(2001),
        &healing,
        target,
    );
    let extras = accepted_hit(
        &mut state,
        target,
        crate::intent::DamageSource::Projectile {
            projectile: EntityId(2001),
            direct_type_id: 0,
            owner,
            velocity: [1.0, 0.0],
            punch: 0,
            flame: false,
        },
        &mut 5.0,
    );
    assert!(extras.is_empty());
    assert_eq!(
        state.players[&target].vitals.health, 14.0,
        "healing is not hurting PvP"
    );
}

#[test]
fn owner_death_removes_only_owned_pearls_when_the_gamerule_is_true() {
    for vanish in [true, false] {
        for player_owner in [true, false] {
            let (mut state, player) = fixture();
            state
                .gamerules
                .insert("ender_pearls_vanish_on_death".into(), vanish.to_string());
            let owner = if player_owner {
                player
            } else {
                mob(&mut state, "minecraft:cow", [0.0, 201.0, 0.0])
            };
            let owner_slot = state.entities.slot_of(owner).unwrap();
            let uuid = state.entities.row(owner_slot).uuid;
            let mut owned = Vec::new();
            for _ in 0..2 {
                let record = ProjectileState::new(
                    ProjectileKind::EnderPearl,
                    Some(uuid),
                    item(&state, "minecraft:ender_pearl"),
                );
                owned.push(launch(&mut state, record, [0.0, 210.0, 0.0], [1.0, 0.0, 0.0]).unwrap());
            }
            let mut retained = Vec::new();
            for (kind, owner_uuid) in [
                (ProjectileKind::EnderPearl, None),
                (ProjectileKind::EnderPearl, Some(Uuid::new_v4())),
                (ProjectileKind::Arrow, Some(uuid)),
            ] {
                let record = ProjectileState::new(kind, owner_uuid, ItemStack::EMPTY);
                retained
                    .push(launch(&mut state, record, [0.0, 210.0, 0.0], [1.0, 0.0, 0.0]).unwrap());
            }
            let mut queued = Vec::new();
            super::super::queue_owned_pearls_on_deaths(&state, &[owner, owner], &mut queued);
            assert_eq!(queued.len(), if vanish { 2 } else { 0 });
            if vanish {
                assert_ne!(
                    queued[0], queued[1],
                    "duplicate deaths cannot duplicate pearl slots"
                );
            }
            state.merge_damage.push((
                owner_slot,
                crate::tick::DamagePiece {
                    physical: None,
                    custom_armor: false,
                    amount: f32::MAX,
                    kind: DamageKind::Void,
                    source: None,
                },
            ));
            let (deaths, events, _, removed) = state.commit_merge_effects();
            assert_eq!(deaths, [owner]);
            assert!(events.iter().any(|event| matches!(
                event, crate::boundary::EntityEvent::Death { entity } if *entity == owner
            )));
            for pearl in owned {
                assert_eq!(
                    state.entities.slot_of_any(pearl).is_none(),
                    vanish,
                    "vanish={vanish} player_owner={player_owner}"
                );
                assert_eq!(removed.contains(&pearl), vanish, "same-commit tombstone");
            }
            for id in retained {
                assert!(state.entities.slot_of(id).is_some());
                assert!(!removed.contains(&id));
            }
            assert_eq!(removed.contains(&owner), !player_owner);
            if player_owner {
                assert!(
                    !state
                        .entities
                        .body(state.entities.slot_of_any(owner).unwrap())
                        .alive
                );
            }
            // A later commit cannot emit the same pearl tombstones again.
            assert!(state.commit_merge_effects().3.is_empty());
        }
    }
}

#[test]
fn trial_wind_charge_player_throw_boosts_owner_and_protects_next_landing() {
    let (mut state, player) = fixture();
    state.world.ensure_blocking(&state.desired_chunks());
    let held = item(&state, "minecraft:wind_charge");
    state.players.get_mut(&player).unwrap().inventory.slots[0] = held;
    let (output, _, _) = run_tick(
        &mut state,
        &[PlayerInput::UseItem {
            connection: ConnectionId(1),
            hand: crate::boundary::Hand::Main,
            yaw: 0.0,
            pitch: 90.0,
            sequence: 1,
        }],
    );
    let id = output
        .changes
        .entities_spawned
        .iter()
        .copied()
        .find(|id| {
            state.entities.slot_of(*id).is_some_and(|slot| {
                state
                    .entities
                    .row(slot)
                    .projectile
                    .as_ref()
                    .is_some_and(|record| record.kind == ProjectileKind::WindCharge)
            })
        })
        .expect("player-thrown wind charge");
    let slot = state.entities.slot_of(id).unwrap();
    let mut record = state.entities.row(slot).projectile.clone().unwrap();
    let player_slot = state.entities.slot_of(player).unwrap();
    let at = state.entities.pos(player_slot);
    state.players.get_mut(&player).unwrap().fall_distance = 8.0;
    assert!(impact(
        &mut state,
        &mut Vec::new(),
        id,
        &mut record,
        None,
        at
    ));
    assert!(
        state.entities.vel(player_slot)[1] > 0.0,
        "thrower receives upward self-knockback"
    );
    let player = state.players.get_mut(&player).unwrap();
    assert_eq!(player.fall_distance, 0.0);
    assert!(player.current_impulse_impact_pos.is_some());
    assert_eq!(player.fall(-10.0, false, false), 0.0);
    assert_eq!(player.fall(0.0, true, false), 0.0);
    assert!(player.current_impulse_impact_pos.is_none());
    player.fall(-10.0, false, false);
    assert!(
        player.fall(0.0, true, false) > 0.0,
        "protection is consumed once"
    );
}

#[test]
fn trial_breeze_rejects_wind_charge_direct_damage() {
    let (mut state, _) = fixture();
    let breeze = mob(&mut state, "minecraft:breeze", [0.0, 201.0, 0.0]);
    for kind in [ProjectileKind::WindCharge, ProjectileKind::BreezeWindCharge] {
        let record = ProjectileState::new(kind, None, ItemStack::EMPTY);
        let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
        let mut intents = Vec::new();
        assert!(impact(
            &mut state,
            &mut intents,
            id,
            &mut record,
            Some(breeze),
            [0.0, 201.0, 0.0]
        ));
        assert!(damage_to(&intents, breeze).is_empty());
    }
}

#[test]
fn weather_gap_lightning_transforms_charges_and_damages_items_and_turtles() {
    let (mut state, _) = fixture();
    let at = [0.5, 201.0, 0.5];
    let creeper = mob(&mut state, "minecraft:creeper", at);
    let pig = mob(&mut state, "minecraft:pig", at);
    let mooshroom = mob(&mut state, "minecraft:mooshroom", at);
    let turtle = mob(&mut state, "minecraft:turtle", at);
    let stack = item(&state, "minecraft:stone");
    let dropped = crate::tick::spawn_item_entity(&mut state, at, stack);
    let mut intents = bolt_damage(&mut state, None, at);
    assert_eq!(damage_to(&intents, dropped), [(5.0, DamageKind::Lightning)]);
    state.merge(&mut intents);
    let (deaths, _, _, _) = state.commit_merge_effects();
    assert!(deaths.contains(&turtle));
    assert!(deaths.contains(&dropped));
    assert_eq!(entity_name(&state, pig), Some("minecraft:zombified_piglin"));
    let pig_row = state.entities.row(state.entities.slot_of(pig).unwrap());
    assert_eq!(
        pig_row.appearance.equipment[0],
        Some(item(&state, "minecraft:golden_sword"))
    );
    let row = state.entities.row(state.entities.slot_of(creeper).unwrap());
    assert!(row.entity_life.charged_lightning);
    assert!(
        row.appearance
            .extra
            .contains(&(17, crate::boundary::EntityDataValue::Boolean(true)))
    );
    assert_eq!(row.health, 15.0);
    let row = state
        .entities
        .row(state.entities.slot_of(mooshroom).unwrap());
    assert!(row.entity_life.brown_mooshroom);
    assert_eq!(row.health, 20.0);
    let fields = crate::entity_life::to_fields(
        &state
            .entities
            .row(state.entities.slot_of(creeper).unwrap())
            .entity_life,
        0,
    );
    assert!(fields.contains(&("powered".into(), mc_nbt::Tag::Byte(1))));
    let record = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:creeper".into(),
        extra_fields: fields,
        ..Default::default()
    };
    let encoded = mc_data::entity_nbt::entity_to_fields(&state.registry.items, &record).unwrap();
    let decoded = mc_data::entity_nbt::entity_from_fields(
        &state.registry.items,
        Default::default(),
        &encoded,
    )
    .unwrap();
    assert!(
        crate::entity_life::from_record(&decoded, false)
            .0
            .charged_lightning
    );
    assert!(crate::weather::lightning::strike_entity(
        &mut state, mooshroom
    ));
    assert!(
        !state
            .entities
            .row(state.entities.slot_of(mooshroom).unwrap())
            .entity_life
            .brown_mooshroom
    );
}

#[test]
fn pearl_landing_spawns_endermite_on_winning_roll() {
    // ENDER-PEARL-TELEPORT-SPAWN through the impact path: a winning 5%
    // roll spawns an endermite at the owner's pre-teleport position.
    let (mut state, owner) = fixture();
    state.tick = 1;
    let owner_uuid = state.entities.row(state.entities.slot_of(owner).unwrap()).uuid;
    let tick = (1_u64..500)
        .find(|tick| {
            crate::natural_spawn::Rng(owner_uuid.as_u128() as u64 ^ tick).below(20) == 0
        })
        .expect("winning tick");
    state.tick = tick;
    let record = ProjectileState::new(
        ProjectileKind::EnderPearl,
        Some(owner_uuid),
        item(&state, "minecraft:ender_pearl"),
    );
    let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    let mut intents = Vec::new();
    assert!(impact(&mut state, &mut intents, id, &mut record, None, [5.0, 201.0, 0.0]));
    assert!(
        state
            .projectiles
            .spawned
            .iter()
            .any(|spawned| entity_name(&state, *spawned) == Some("minecraft:endermite")),
        "winning pearl roll must spawn an endermite"
    );
}

#[test]
fn snowball_breaks_end_crystal_through_zero_damage_path() {
    // ZERO-DAMAGE-IMPACT through the impact path: zero-damage contact
    // still destroys an end crystal via the shared hit path.
    let (mut state, owner) = fixture();
    let crystal = mob(&mut state, "minecraft:end_crystal", [2.0, 201.0, 0.0]);
    let owner_uuid = state.entities.row(state.entities.slot_of(owner).unwrap()).uuid;
    let record = ProjectileState::new(
        ProjectileKind::Snowball,
        Some(owner_uuid),
        item(&state, "minecraft:snowball"),
    );
    let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    let mut intents = Vec::new();
    assert!(impact(
        &mut state,
        &mut intents,
        id,
        &mut record,
        Some(crystal),
        [2.0, 201.0, 0.0]
    ));
    assert!(
        !damage_to(&intents, crystal).is_empty(),
        "snowball contact must route damage to an end crystal"
    );
}

#[test]
fn splash_water_douses_burning_entity() {
    // WATER-IMPACT-SPLASH through the impact path: a splash water bottle
    // extinguishes a burning entity instead of resolving potion effects.
    let (mut state, owner) = fixture();
    let target = mob(&mut state, "minecraft:cow", [2.0, 201.0, 0.0]);
    let target_slot = state.entities.slot_of(target).unwrap();
    state.entities.body_mut(target_slot).burn_ticks = 100;
    let owner_uuid = state.entities.row(state.entities.slot_of(owner).unwrap()).uuid;
    let record = ProjectileState::new(
        ProjectileKind::Potion,
        Some(owner_uuid),
        potion(&state, "minecraft:splash_potion", "minecraft:water"),
    );
    let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    assert!(impact(
        &mut state,
        &mut Vec::new(),
        id,
        &mut record,
        Some(target),
        [2.0, 201.0, 0.0]
    ));
    assert_eq!(state.entities.body(state.entities.slot_of(target).unwrap()).burn_ticks, 0);
}

#[test]
fn custom_only_splash_applies_custom_effect() {
    // CUSTOM-POTION-EFFECTS through the impact path: a custom-only splash
    // (no base potion) applies its custom effects to the direct hit.
    let (mut state, owner) = fixture();
    let mut stack = item(&state, "minecraft:splash_potion");
    stack.extended_mut().potion_custom_effects.push(
        mc_data::item_stack::PotionEffect {
            id: mc_data::Identifier::parse("minecraft:speed").unwrap(),
            amplifier: 0,
            duration: 600,
            ambient: false,
            show_particles: true,
            show_icon: true,
            hidden: None,
        },
    );
    let owner_uuid = state.entities.row(state.entities.slot_of(owner).unwrap()).uuid;
    let record = ProjectileState::new(ProjectileKind::Potion, Some(owner_uuid), stack);
    let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    assert!(impact(
        &mut state,
        &mut Vec::new(),
        id,
        &mut record,
        Some(owner),
        [10.0, 201.0, 0.0]
    ));
    assert!(
        state.players[&owner].active_effects.get(crate::effects::EffectId::Speed).is_some(),
        "custom-only splash must apply its custom speed effect"
    );
}

#[test]
fn lingering_healing_cloud_heals_other_player_with_pvp_off() {
    // LINGERING-FRIENDLY-PVP through the cloud path: a healing lingering
    // cloud still heals another player while PvP is disabled.
    let (mut state, _) = fixture();
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(2),
            uuid: Uuid::new_v4(),
            name: "cloud-friend".into(),
            permission_level: 0,
        }],
    );
    let friend = state.connections.entity_of(ConnectionId(2)).unwrap();
    let friend_slot = state.entities.slot_of(friend).unwrap();
    state.entities.set_pos(friend_slot, [1.0, 201.0, 0.0]);
    state.players.get_mut(&friend).unwrap().vitals.health = 10.0;
    state.entities.row_mut(friend_slot).health = 10.0;
    state.gamerules.insert("pvp".into(), "false".into());
    let record = ProjectileState::new(
        ProjectileKind::LingeringPotion,
        None,
        potion(&state, "minecraft:lingering_potion", "minecraft:healing"),
    );
    let (bottle, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    assert!(impact(
        &mut state,
        &mut Vec::new(),
        bottle,
        &mut record,
        None,
        [0.0, 201.0, 0.0]
    ));
    remove(&mut state, bottle);
    let cloud_id = state
        .projectiles
        .spawned
        .iter()
        .copied()
        .find(|id| {
            state.entities.slot_of(*id).is_some_and(|slot| {
                state.entities.row(slot).projectile.as_ref().is_some_and(|record| {
                    record.kind == ProjectileKind::AreaEffectCloud
                })
            })
        })
        .expect("lingering cloud");
    let cloud_slot = state.entities.slot_of(cloud_id).unwrap();
    let mut cloud = *state.entities.row_mut(cloud_slot).projectile.take().unwrap();
    let friend_health = |state: &TickState| state.players[&friend].vitals.health;
    let before = friend_health(&state);
    for _ in 0..12 {
        state.tick += 5;
        cloud.age += 5;
        let mut intents = Vec::new();
        if !step(&mut state, &mut intents, cloud_id, &mut cloud) {
            break;
        }
        if friend_health(&state) > before {
            return;
        }
    }
    panic!("healing cloud must heal another player with pvp=false");
}

#[test]
fn splash_rejects_targets_above_the_vertical_band() {
    // POTION-TARGET-GEOMETRY through the splash path: a target 3 blocks
    // above the burst (outside the vertical band) is not splashed.
    let (mut state, owner) = fixture();
    let above = mob(&mut state, "minecraft:cow", [0.0, 204.0, 0.0]);
    let owner_uuid = state.entities.row(state.entities.slot_of(owner).unwrap()).uuid;
    let record = ProjectileState::new(
        ProjectileKind::Potion,
        Some(owner_uuid),
        potion(&state, "minecraft:splash_potion", "minecraft:harming"),
    );
    let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    let mut intents = Vec::new();
    assert!(impact(
        &mut state,
        &mut intents,
        id,
        &mut record,
        None,
        [0.0, 201.0, 0.0]
    ));
    assert!(
        damage_to(&intents, above).is_empty(),
        "cow 3 blocks above the burst must be outside the splash band"
    );
}

#[test]
fn splash_regeneration_does_not_apply_to_undead() {
    // MOB-IMMUNITY (undead) through the splash path: regeneration from a
    // splash potion is rejected by zombies.
    let (mut state, owner) = fixture();
    let zombie = mob(&mut state, "minecraft:zombie", [2.0, 201.0, 0.0]);
    let owner_uuid = state.entities.row(state.entities.slot_of(owner).unwrap()).uuid;
    let record = ProjectileState::new(
        ProjectileKind::Potion,
        Some(owner_uuid),
        potion(&state, "minecraft:splash_potion", "minecraft:regeneration"),
    );
    let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    assert!(impact(
        &mut state,
        &mut Vec::new(),
        id,
        &mut record,
        Some(zombie),
        [2.0, 201.0, 0.0]
    ));
    let zombie_slot = state.entities.slot_of(zombie).unwrap();
    assert!(
        state.entities.row(zombie_slot).active_effects.entries.is_empty(),
        "undead must reject regeneration, got {:?}",
        state.entities.row(zombie_slot).active_effects.entries
    );
}

#[test]
fn base_plus_custom_splash_applies_both_effects() {
    // CUSTOM-POTION-EFFECTS composition (PotionContents.getAllEffects):
    // base potion effects concatenated with custom effects — a
    // swiftness-plus-custom-strength stack must yield both, not base alone.
    let (mut state, owner) = fixture();
    let mut stack = potion(&state, "minecraft:splash_potion", "minecraft:swiftness");
    stack.extended_mut().potion_custom_effects.push(
        mc_data::item_stack::PotionEffect {
            id: mc_data::Identifier::parse("minecraft:strength").unwrap(),
            amplifier: 0,
            duration: 600,
            ambient: false,
            show_particles: true,
            show_icon: true,
            hidden: None,
        },
    );
    let owner_uuid = state.entities.row(state.entities.slot_of(owner).unwrap()).uuid;
    let record = ProjectileState::new(ProjectileKind::Potion, Some(owner_uuid), stack);
    let (id, mut record) = launch_record(&mut state, record, [0.0, 201.0, 0.0]);
    assert!(impact(
        &mut state,
        &mut Vec::new(),
        id,
        &mut record,
        Some(owner),
        [10.0, 201.0, 0.0]
    ));
    let active = &state.players[&owner].active_effects;
    assert!(
        active.get(crate::effects::EffectId::Speed).is_some()
            && active.get(crate::effects::EffectId::Strength).is_some(),
        "base+custom splash must apply both swiftness and custom strength"
    );
}
