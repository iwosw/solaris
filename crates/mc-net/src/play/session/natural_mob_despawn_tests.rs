use super::tests::profile;
use super::*;
use std::cell::Cell;
use std::collections::HashSet;
use tokio::sync::mpsc;

thread_local! {
    static REMOVE_AFTER_PROJECTION: Cell<Option<EntityId>> = const { Cell::new(None) };
    static MOVE_AFTER_PROJECTION: Cell<Option<(EntityId, Vec3)>> = const { Cell::new(None) };
    static PAUSE_BEFORE_PROJECTION: std::cell::RefCell<Option<(
        std::sync::mpsc::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    )>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn after_projection(inner: &mut SessionEntityGuards<'_>) {
    REMOVE_AFTER_PROJECTION.with(|pending| {
        if let Some(id) = pending.take() {
            let expected = inner.entities.snapshot(id).unwrap();
            assert!(inner.entities.remove_if_current(expected).is_some());
        }
    });
    MOVE_AFTER_PROJECTION.with(|pending| {
        if let Some((id, position)) = pending.take() {
            let expected = inner.entities.snapshot(id).unwrap();
            let mut next = expected.clone();
            next.position = position;
            assert!(inner.entities.replace_snapshot_if_current(expected, next));
        }
    });
}

pub(super) fn before_projection() {
    if let Some((reached, resume)) =
        PAUSE_BEFORE_PROJECTION.with(|pending| pending.borrow_mut().take())
    {
        reached.send(()).expect("projection pause receiver");
        resume.recv().expect("projection resume sender");
    }
}

fn distant_natural_zombie() -> (SessionRegistry, EntityId, mpsc::Receiver<OutboundCommand>) {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("DespawnRace"),
        name: "DespawnRace".to_owned(),
    };
    let (_session, _) = registry.register(
        &profile,
        (0, 0),
        8,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        150,
        "minecraft:zombie".to_owned(),
        Vec3::new(240.5, 64.0, 0.5),
    );
    let id = registry.persisted_entity_records()[0].snapshot.id;
    {
        let mut inner = registry.lock_inner("install natural race fixture");
        inner.natural_hostile_mobs.insert(id);
        inner.natural_mob_no_action_since_tick.insert(id, 0);
    }
    (registry, id, _rx)
}

#[test]
fn vanished_after_projection_clears_natural_idle_clock() {
    let (registry, id, _rx) = distant_natural_zombie();
    REMOVE_AFTER_PROJECTION.with(|pending| pending.set(Some(id)));
    assert_eq!(registry.tick_natural_mob_despawn(700).removed(), 0);
    assert!(registry.persisted_entity_records().is_empty());
    let inner = registry.lock_inner("verify vanished natural entity clock");
    assert!(!inner.natural_mob_no_action_since_tick.contains_key(&id));
}

#[test]
fn session_join_progresses_during_despawn_projection_and_protects_nearby_mob() {
    let registry = std::sync::Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(8);
    registry.register(
        &LoggedInProfile {
            uuid: crate::login::offline_uuid("DistantDespawnObserver"),
            name: "DistantDespawnObserver".to_owned(),
        },
        (0, 0),
        8,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        150,
        "minecraft:zombie".to_owned(),
        Vec3::new(240.5, 64.0, 0.5),
    );
    let id = registry.persisted_entity_records()[0].snapshot.id;
    registry
        .lock_inner("install natural despawn join fixture")
        .natural_hostile_mobs
        .insert(id);
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let worker_registry = std::sync::Arc::clone(&registry);
    let worker = std::thread::spawn(move || {
        PAUSE_BEFORE_PROJECTION.with(|pending| {
            *pending.borrow_mut() = Some((reached_tx, resume_rx));
        });
        worker_registry.tick_natural_mob_despawn(700).removed()
    });
    reached_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("despawn reached owner projection");
    let (near_tx, _near_rx) = mpsc::channel(8);
    let (joined_tx, joined_rx) = std::sync::mpsc::channel();
    let joining_registry = std::sync::Arc::clone(&registry);
    let joining = std::thread::spawn(move || {
        joining_registry.register(
            &LoggedInProfile {
                uuid: crate::login::offline_uuid("NearbyDespawnObserver"),
                name: "NearbyDespawnObserver".to_owned(),
            },
            (15, 0),
            8,
            HashSet::new(),
            near_tx,
            PlayerPose::new(240.5, 64.0, 0.5),
        );
        joined_tx.send(()).expect("join progress receiver");
    });
    let progressed = joined_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .is_ok();
    resume_tx.send(()).expect("release paused projection");
    let removed = worker.join().expect("despawn worker");
    joining.join().expect("joining session");
    assert!(
        progressed,
        "owner projection must not block session registration"
    );
    assert_eq!(removed, 0, "the newly nearby player prevents despawn");
    assert!(registry.server_entity_snapshot(id).is_some());
}

#[test]
fn movement_after_projection_prevents_stale_despawn() {
    let (registry, id, _rx) = distant_natural_zombie();
    let nearby = Vec3::new(0.5, 64.0, 0.5);
    MOVE_AFTER_PROJECTION.with(|pending| pending.set(Some((id, nearby))));

    assert_eq!(registry.tick_natural_mob_despawn(700).removed(), 0);
    assert_eq!(
        registry.server_entity_snapshot(id).unwrap().position,
        nearby
    );
}

#[test]
fn natural_mob_hard_despawn_respects_exact_distance_and_persistent_category() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (player, _) = registry.register(
        &profile("NaturalDespawnObserver"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    for (type_id, type_name, position) in [
        (150, "minecraft:zombie", Vec3::new(128.5, 64.0, 0.5)),
        (150, "minecraft:zombie", Vec3::new(128.500_1, 64.0, 0.5)),
        (26, "minecraft:chicken", Vec3::new(200.5, 64.0, 0.5)),
    ] {
        let _ = registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            type_id,
            type_name.to_owned(),
            position,
        );
    }
    let (boundary_zombie, distant_zombie, distant_chicken) = {
        let entities = registry.lock_entities("read hard despawn fixtures");
        let find = |name: &str, x: f64| {
            entities
                .snapshots()
                .find(|entity| entity.type_name == name && (entity.position.x - x).abs() < 1.0e-9)
                .map(|entity| entity.id)
                .expect("hard despawn fixture")
        };
        (
            find("minecraft:zombie", 128.5),
            find("minecraft:zombie", 128.500_1),
            find("minecraft:chicken", 200.5),
        )
    };
    {
        let mut inner = registry.lock_inner("mark hard despawn natural fixtures");
        inner.natural_hostile_mobs.insert(boundary_zombie);
        inner.natural_hostile_mobs.insert(distant_zombie);
        inner.natural_ground_mobs.insert(distant_chicken);
    }

    assert_eq!(registry.tick_natural_mob_despawn(20).removed(), 1);
    assert!(registry.server_entity_snapshot(boundary_zombie).is_some());
    assert!(registry.server_entity_snapshot(distant_zombie).is_none());
    assert!(registry.server_entity_snapshot(distant_chicken).is_some());
    let inner = registry.lock_inner("verify hard despawn indexes");
    assert!(inner.natural_hostile_mobs.contains(&boundary_zombie));
    assert!(!inner.natural_hostile_mobs.contains(&distant_zombie));
    assert!(inner.natural_ground_mobs.contains(&distant_chicken));
}

#[test]
fn water_ambient_hard_despawn_uses_sixty_four_block_distance() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (player, _) = registry.register(
        &profile("WaterDespawnObserver"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    let _ = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        110,
        "minecraft:salmon".to_owned(),
        Vec3::new(64.500_1, 64.0, 0.5),
    );
    let salmon = registry
        .lock_entities("read water ambient despawn fixture")
        .snapshots()
        .find(|entity| entity.type_name == "minecraft:salmon")
        .map(|entity| entity.id)
        .expect("salmon fixture");
    registry
        .lock_inner("mark water ambient natural fixture")
        .natural_aquatic_mobs
        .insert(salmon);

    assert_eq!(registry.tick_natural_mob_despawn(20).removed(), 1);
    assert!(registry.server_entity_snapshot(salmon).is_none());
}

#[test]
fn natural_mob_soft_despawn_requires_six_hundred_idle_ticks_and_exact_roll() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (player, _) = registry.register(
        &profile("SoftDespawnObserver"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    let _ = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        150,
        "minecraft:zombie".to_owned(),
        Vec3::new(40.5, 64.0, 0.5),
    );
    let zombie = registry
        .lock_entities("read soft despawn fixture")
        .snapshots()
        .find(|entity| entity.type_name == "minecraft:zombie")
        .expect("soft despawn zombie");
    {
        let mut inner = registry.lock_inner("mark soft despawn natural fixture");
        inner.natural_hostile_mobs.insert(zombie.id);
        inner.natural_mob_no_action_since_tick.insert(zombie.id, 0);
    }

    assert_eq!(registry.tick_natural_mob_despawn(600).removed(), 0);
    assert!(registry.server_entity_snapshot(zombie.id).is_some());
    let roll_tick = (601..100_000)
        .find(|tick| super::entity_lifecycle::natural_mob_soft_despawn_roll(zombie.uuid, *tick))
        .expect("bounded deterministic range contains a one-in-800 soft despawn roll");
    assert_eq!(registry.tick_natural_mob_despawn(roll_tick).removed(), 1);
    assert!(registry.server_entity_snapshot(zombie.id).is_none());
}

#[test]
fn natural_mob_soft_despawn_idle_clock_resets_near_player_and_after_damage() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (player, _) = registry.register(
        &profile("SoftResetObserver"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    for position in [Vec3::new(31.5, 64.0, 0.5), Vec3::new(40.5, 64.0, 0.5)] {
        let _ = registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            150,
            "minecraft:zombie".to_owned(),
            position,
        );
    }
    let (near, damaged) = {
        let entities = registry.lock_entities("read soft reset fixtures");
        let mut zombies = entities
            .snapshots()
            .filter(|entity| entity.type_name == "minecraft:zombie")
            .collect::<Vec<_>>();
        zombies.sort_by(|left, right| left.position.x.total_cmp(&right.position.x));
        (zombies[0].clone(), zombies[1].clone())
    };
    {
        let mut entities = registry.lock_entities("record soft reset damage fixture");
        let expected = entities
            .snapshot(damaged.id)
            .expect("damaged zombie remains current");
        let mut next = expected.clone();
        next.retained.last_damage_tick = Some(650);
        assert!(entities.replace_snapshot_if_current(expected, next));
    }
    {
        let mut inner = registry.lock_inner("mark soft reset natural fixtures");
        inner.natural_hostile_mobs.extend([near.id, damaged.id]);
        inner.natural_mob_no_action_since_tick.insert(near.id, 0);
        inner.natural_mob_no_action_since_tick.insert(damaged.id, 0);
    }

    assert_eq!(registry.tick_natural_mob_despawn(700).removed(), 0);
    let inner = registry.lock_inner("verify soft despawn idle resets");
    assert_eq!(
        inner.natural_mob_no_action_since_tick.get(&near.id),
        Some(&700)
    );
    assert_eq!(
        inner.natural_mob_no_action_since_tick.get(&damaged.id),
        Some(&650)
    );
}

#[test]
fn natural_mob_category_sets_stay_mutually_exclusive_for_the_despawn_scan() {
    let registry = SessionRegistry::new();
    let mut inner = registry.lock_inner("fence natural category disjointness");
    assert!(super::entity_lifecycle::natural_mob_category_sets_disjoint(
        &inner
    ));
    let overlap = EntityId(4_242_424);
    inner.natural_hostile_mobs.insert(overlap);
    inner.natural_ground_mobs.insert(overlap);
    assert!(!super::entity_lifecycle::natural_mob_category_sets_disjoint(&inner));
}

#[test]
fn natural_mob_despawn_clears_idle_clock_for_stale_category_ids() {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let (player, _) = registry.register(
        &profile("StaleDespawnObserver"),
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    let stale = EntityId(9_876_543);
    {
        let mut inner = registry.lock_inner("mark stale natural fixture");
        inner.natural_hostile_mobs.insert(stale);
        inner.natural_mob_no_action_since_tick.insert(stale, 0);
    }

    assert_eq!(registry.tick_natural_mob_despawn(20).removed(), 0);
    let inner = registry.lock_inner("verify stale natural idle clock cleanup");
    assert!(inner.natural_hostile_mobs.contains(&stale));
    assert!(!inner.natural_mob_no_action_since_tick.contains_key(&stale));
}

#[test]
fn natural_mob_despawn_outcome_is_independent_of_category_iteration_order() {
    const REMOTE_ZOMBIE_X: f64 = 240.5;
    const REMOTE_SALMON_X: f64 = 90.5;
    fn seeded(name: &str) -> (SessionRegistry, Vec<EntityId>) {
        let registry = SessionRegistry::new();
        let (tx, _rx) = mpsc::channel(8);
        let (player, _) = registry.register(
            &profile(name),
            (0, 0),
            2,
            HashSet::from([(0, 0)]),
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        assert!(registry.mark_loaded(player, (0, 0)).is_empty());
        let mut ids = Vec::new();
        for (type_id, type_name, x) in [
            (150, "minecraft:zombie", 20.5),
            (150, "minecraft:zombie", 40.5),
            (150, "minecraft:zombie", 100.5),
            (150, "minecraft:zombie", REMOTE_ZOMBIE_X),
            (26, "minecraft:chicken", 31.5),
            (26, "minecraft:chicken", 100.5),
            (110, "minecraft:salmon", 32.5),
            (110, "minecraft:salmon", 64.5),
            (110, "minecraft:salmon", REMOTE_SALMON_X),
        ] {
            registry.spawn_command_entity(
                &SimulationAuthority::for_test(),
                type_id,
                type_name.to_owned(),
                Vec3::new(x, 64.0, 0.5),
            );
        }
        {
            let entities = registry.lock_entities("read natural despawn fixtures");
            for (type_name, x) in [
                ("minecraft:zombie", 20.5),
                ("minecraft:zombie", 40.5),
                ("minecraft:zombie", 100.5),
                ("minecraft:zombie", REMOTE_ZOMBIE_X),
                ("minecraft:chicken", 31.5),
                ("minecraft:chicken", 100.5),
                ("minecraft:salmon", 32.5),
                ("minecraft:salmon", 64.5),
                ("minecraft:salmon", REMOTE_SALMON_X),
            ] {
                ids.push(
                    entities
                        .snapshots()
                        .find(|entity| {
                            entity.type_name == type_name && (entity.position.x - x).abs() < 1.0e-9
                        })
                        .map(|entity| entity.id)
                        .unwrap_or_else(|| panic!("missing {type_name} fixture at {x}")),
                );
            }
        }
        (registry, ids)
    }
    let (first, first_ids) = seeded("DespawnOrderAlice");
    let (second, second_ids) = seeded("DespawnOrderBob");
    for (registry, ids) in [(&first, &first_ids), (&second, &second_ids)] {
        let mut inner = registry.lock_inner("mark natural category fixtures");
        for (index, id) in ids.iter().enumerate() {
            match index {
                0..=3 => {
                    inner.natural_hostile_mobs.insert(*id);
                }
                4..=5 => {
                    inner.natural_ground_mobs.insert(*id);
                }
                6..=8 => {
                    inner.natural_aquatic_mobs.insert(*id);
                }
                _ => unreachable!("unexpected natural despawn fixture index"),
            }
        }
        assert!(super::entity_lifecycle::natural_mob_category_sets_disjoint(
            &inner
        ));
    }

    assert_eq!(first.tick_natural_mob_despawn(20).removed(), 2);
    assert_eq!(second.tick_natural_mob_despawn(20).removed(), 2);
    let alive_flags = |registry: &SessionRegistry, ids: &[EntityId]| {
        ids.iter()
            .map(|id| registry.server_entity_snapshot(*id).is_some())
            .collect::<Vec<_>>()
    };
    // Zombies hard-despawn only beyond 128 blocks, chickens are the
    // persistent category, salmon hard-despawns beyond 64 blocks with the
    // exact boundary surviving.
    let expected_alive = [true, true, true, false, true, true, true, true, false];
    assert_eq!(alive_flags(&first, &first_ids), expected_alive);
    assert_eq!(alive_flags(&second, &second_ids), expected_alive);
}
