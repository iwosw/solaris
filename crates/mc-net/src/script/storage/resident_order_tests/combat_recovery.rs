//! Combat and recovery contracts: ranged policy, retreat/rally routing, injury and capture.
//!
//! Moved out of `resident_order_tests.rs` by CP-REFACTOR-040 sub-slice 5.
//! Child of `resident_order_tests`: explicit imports only, no glob.
//!
//! Fixture methods (`enable_arrows`, `advance_resident_arrows`, `goal`, `position`, `health`) stay in shared support.

use crate::play::EntityPhysicsStep;
use crate::script::storage::PluginStorage;
use crate::script::storage::resident_orders::DurableResidentOrderChange;
use mc_entity::SpawnEntity;
use mc_entity::Vec3;
use mc_script::ScriptBlockPosition;
use mc_script::ScriptEngagementPolicy;
use mc_script::ScriptHostileCategory;
use mc_script::ScriptOperation;
use mc_script::ScriptOperationFailure;
use mc_script::ScriptOperationPayload;
use mc_script::ScriptOperationRequest;
use mc_script::ScriptOrderMemberState;
use mc_script::ScriptOrderTargetRef;
use mc_script::ScriptResidentOrder;
use mc_script::ScriptResidentOrderOperation;
use mc_script::ScriptResidentOrderResult;
use mc_world::BlockPos;
use mc_world::ChunkPos;

use super::{
    Fixture, OWNER, SURFACE_Y, demobilize_request, formation, gear, members_revision, order_of,
    order_request, seed_gear, seed_gear_in, state_of,
};
/// (A10) An archer consumes one arrow only on launch; the shared projectile
/// kernel hits an authorized target later, while no ammo, a wall and ally policy
/// prevent launch or damage.
#[tokio::test]
async fn archer_ammo_line_of_sight_and_ally_policy_gate_committed_damage() {
    let fixture = Fixture::new(false);
    fixture.enable_arrows();
    let mut storage = fixture.storage();
    let (archer_handle, archer) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let (ally_handle, ally_uuid) = fixture
        .resident(&mut storage, 4, Vec3::new(5.5, 64.0, 5.5))
        .await;
    // The ally is a real resident entity whose spawn path is the regional
    // owner, not a session spawn, so register it in the same chunk index the
    // server maintains. Without that registration no local perception can see
    // it and the ally policy would be untestable.
    let ally_id = fixture
        .sessions
        .resident_entity_snapshots(&[ally_uuid])
        .await
        .into_iter()
        .next()
        .flatten()
        .expect("the ally stays live")
        .id;
    assert!(
        fixture.sessions.track_entity_chunk_for_test(ally_id),
        "the ally is registered for perception"
    );
    let zombie = fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(6.5, 64.0, 4.5)),
            true,
        )
        .expect("zombie spawns");
    let zombie_snapshot = fixture
        .sessions
        .snapshot_entity_for_test(zombie)
        .expect("zombie is tracked");
    let zombie_uuid = zombie_snapshot.uuid;
    assert!(zombie_snapshot.health > 0.0);

    // A hold order perceives over the same local index and issues the
    // server-issued reference the attack order must name.
    let holders = vec![archer_handle.clone()];
    let hold = ScriptResidentOrder::Hold {
        anchor: ScriptBlockPosition::new(4, 64, 4),
        heading_degrees: 0,
        formation: formation(),
        engagement_radius: 16,
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request("hold-1", &holders, &[0], hold.clone()),
        )
        .await;
    let (_, members, _) = order_of(&outcome);
    let perceived = members[0].targets.clone();
    assert!(
        perceived
            .iter()
            .any(|target| target.category == ScriptHostileCategory::Hostile),
        "the hostile is perceived: {perceived:?}"
    );
    assert!(
        !perceived
            .iter()
            .any(|target| { target.position == ScriptBlockPosition::new(5, 64, 5) }),
        "a proximity order offers hostile mobs only, never the allied resident: {perceived:?}"
    );
    let mut hostile_target = perceived
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile reference")
        .clone();

    let attack_order = |targets: Vec<ScriptOrderTargetRef>, policy: ScriptEngagementPolicy| {
        ScriptResidentOrder::Attack { targets, policy }
    };
    let reference = |target: &mc_script::ScriptOrderTarget| {
        ScriptOrderTargetRef::new(
            target.target_ref.clone(),
            target.policy_revision,
            target.expires_revision,
        )
    };
    let policy_with_ally = {
        let mut policy = ScriptEngagementPolicy::new(
            0,
            vec![ally_handle.clone()],
            vec![
                ScriptHostileCategory::Hostile,
                ScriptHostileCategory::OwnedResident,
            ],
        );
        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
        policy
    };
    // A bow with no arrows: the archer engages nothing, no damage, no event.
    // The bow must be equipped, or the resident would simply punch.
    seed_gear(
        &mut storage,
        &archer_handle,
        archer,
        &[("minecraft:bow", 1)],
    );
    let bow_revision = equip_bow(&mut storage, &archer_handle);
    let health_before = fixture.health(zombie_uuid).await;
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "attack-no-ammo",
                &holders,
                &[bow_revision],
                attack_order(vec![reference(&hostile_target)], policy_with_ally.clone()),
            ),
        )
        .await;
    let (_, members, combat) = order_of(&outcome);
    assert!(combat.is_empty(), "no ammo means no committed damage");
    assert_eq!(
        fixture.health(zombie_uuid).await,
        health_before,
        "health is unchanged by a refused engagement"
    );
    hostile_target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("refreshed hostile target")
        .clone();

    // A crowded lane cannot stall the one-target arrow or make it hit a
    // bystander: the restricted shot resolves just its issued entity.
    let mut bystander = None;
    for ordinal in 0..12 {
        let cow = fixture
            .sessions
            .spawn_tracked_entity_for_test(
                SpawnEntity::new(
                    11,
                    "minecraft:cow",
                    Vec3::new(5.0 + f64::from(ordinal) * 0.08, 64.0, 4.5),
                ),
                false,
            )
            .expect("bystander spawns");
        bystander.get_or_insert(cow);
    }
    let bystander = bystander.expect("first bystander");
    let bystander_health = fixture
        .sessions
        .snapshot_entity_for_test(bystander)
        .expect("bystander is live")
        .health;
    // With arrows the resident launches a physical projectile; there is no
    // instant-damage receipt before its flight and impact.
    let ammo_revision = seed_gear(
        &mut storage,
        &archer_handle,
        archer,
        &[("minecraft:arrow", 8)],
    );
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "attack-arrows",
                &holders,
                &[ammo_revision],
                attack_order(vec![reference(&hostile_target)], policy_with_ally.clone()),
            ),
        )
        .await;
    let (_, members, combat) = order_of(&outcome);
    assert!(
        combat.is_empty(),
        "launch is not an impact receipt: {combat:?}"
    );
    assert_eq!(fixture.health(zombie_uuid).await, health_before);
    assert_eq!(
        fixture
            .sessions
            .resident_arrows_for_test(Vec3::new(4.5, 64.0, 4.5))
            .len(),
        1,
        "one physical arrow launched"
    );
    assert_eq!(
        fixture.advance_resident_arrows(1, Vec3::new(4.5, 64.0, 4.5)),
        1
    );
    assert!(
        fixture.health(zombie_uuid).await < health_before,
        "the projectile kernel hit the hostile"
    );
    assert_eq!(
        fixture
            .sessions
            .snapshot_entity_for_test(bystander)
            .expect("bystander remains live")
            .health,
        bystander_health,
        "the physical arrow ignored bystanders in its path"
    );
    assert_eq!(
        arrow_count(&storage, &archer_handle),
        7,
        "exactly one arrow was consumed"
    );
    let after_arrows = members_revision(&outcome);
    hostile_target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("refreshed hostile target")
        .clone();
    let health_after_first = fixture.health(zombie_uuid).await;
    let repeated = fixture
        .execute(
            &mut storage,
            &order_request(
                "attack-same-tick",
                &holders,
                &[after_arrows],
                attack_order(vec![reference(&hostile_target)], policy_with_ally.clone()),
            ),
        )
        .await;
    let (_, members, repeated_combat) = order_of(&repeated);
    assert!(
        repeated_combat.is_empty(),
        "same-tick repeated order cannot bypass attack cooldown"
    );
    assert_eq!(fixture.health(zombie_uuid).await, health_after_first);
    assert_eq!(arrow_count(&storage, &archer_handle), 7);
    let after_repeat = members_revision(&repeated);
    hostile_target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("refreshed hostile target")
        .clone();

    // The ally policy gate. The ally is a real target on the damage path: the
    // control volley, whose policy names no ally, hurts it through the same
    // issued reference; the same reference is refused once the policy covers
    // the ally.
    let neutral_policy = |allies: Vec<String>| {
        let mut policy =
            ScriptEngagementPolicy::new(1, allies, vec![ScriptHostileCategory::NeutralAnimal]);
        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
        policy
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "attack-observe-ally",
                &holders,
                &[after_repeat],
                attack_order(vec![reference(&hostile_target)], neutral_policy(Vec::new())),
            ),
        )
        .await;
    let (_, members, combat) = order_of(&outcome);
    assert!(
        combat.is_empty(),
        "a policy permitting neutral animals only never shoots the hostile: {combat:?}"
    );
    let mut ally_target = members[0]
        .targets
        .iter()
        .find(|target| target.position == ScriptBlockPosition::new(5, 64, 5))
        .expect("the adjacent resident is issued when no ally excludes it")
        .clone();

    let after_observe = members_revision(&outcome);
    let ally_health = fixture.health(ally_uuid).await;
    fixture.sessions.synchronize_entity_lifecycle_epoch(20);
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "attack-ally-control",
                &holders,
                &[after_observe],
                attack_order(vec![reference(&ally_target)], neutral_policy(Vec::new())),
            ),
        )
        .await;
    let (_, members, combat) = order_of(&outcome);
    assert!(
        combat.is_empty(),
        "the projectile has not landed: {combat:?}"
    );
    assert_eq!(fixture.health(ally_uuid).await, ally_health);
    assert_eq!(
        fixture.advance_resident_arrows(21, Vec3::new(4.5, 64.0, 4.5)),
        1
    );
    assert!(
        fixture.health(ally_uuid).await < ally_health,
        "the issued reference really addresses the resident"
    );
    let after_control = members_revision(&outcome);
    ally_target = members[0]
        .targets
        .iter()
        .find(|target| target.position == ScriptBlockPosition::new(5, 64, 5))
        .expect("the neutral resident target is refreshed")
        .clone();

    let ally_health = fixture.health(ally_uuid).await;
    let arrows_before = arrow_count(&storage, &archer_handle);
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "attack-ally-gated",
                &holders,
                &[after_control],
                attack_order(vec![reference(&ally_target)], {
                    let mut policy = ScriptEngagementPolicy::new(
                        1,
                        vec![ally_handle.clone()],
                        vec![
                            ScriptHostileCategory::Hostile,
                            ScriptHostileCategory::NeutralAnimal,
                        ],
                    );
                    policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                    policy
                }),
            ),
        )
        .await;
    let (_, members, combat) = order_of(&outcome);
    assert!(
        members[0]
            .targets
            .iter()
            .any(|target| target.category == ScriptHostileCategory::Hostile),
        "the hostile is still perceived: {:?}",
        members[0].targets
    );
    assert!(
        !members[0]
            .targets
            .iter()
            .any(|target| target.position == ScriptBlockPosition::new(5, 64, 5)),
        "an ally named by its durable handle is never issued as a target: {:?}",
        members[0].targets
    );
    assert!(
        combat.is_empty(),
        "an allied resident is never hit: {combat:?}"
    );
    assert_eq!(
        fixture.health(ally_uuid).await,
        ally_health,
        "the allied resident keeps its health"
    );
    assert_eq!(
        arrow_count(&storage, &archer_handle),
        arrows_before,
        "a refused volley consumes no ammunition"
    );

    // A wall between the archer and the hostile stops the shot.
    let walled = Fixture::new(true);
    walled.enable_arrows();
    let mut walled_storage = walled.storage();
    let (walled_handle, walled_archer) = walled
        .resident(&mut walled_storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    seed_gear(
        &mut walled_storage,
        &walled_handle,
        walled_archer,
        &[("minecraft:bow", 1), ("minecraft:arrow", 4)],
    );
    let walled_revision = equip_bow(&mut walled_storage, &walled_handle);
    let walled_holders = vec![walled_handle.clone()];
    let walled_zombie = walled
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(12.5, 64.0, 4.5)),
            true,
        )
        .expect("the hostile behind the wall spawns");
    let walled_uuid = walled
        .sessions
        .snapshot_entity_for_test(walled_zombie)
        .expect("the hostile behind the wall is tracked")
        .uuid;
    let outcome = walled
        .execute(
            &mut walled_storage,
            &order_request("wall-hold", &walled_holders, &[walled_revision], hold),
        )
        .await;
    let (_, members, _) = order_of(&outcome);
    let walled_target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("the hostile is perceived through the wall")
        .clone();
    let walled_health = walled.health(walled_uuid).await;
    let walled_order_revision = members_revision(&outcome);
    let walled_order_revision = if walled_order_revision == 0 {
        1
    } else {
        walled_order_revision
    };
    let walled_policy = {
        let mut policy =
            ScriptEngagementPolicy::new(0, Vec::new(), vec![ScriptHostileCategory::Hostile]);
        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
        policy
    };
    let outcome = walled
        .execute(
            &mut walled_storage,
            &order_request(
                "wall-attack",
                &walled_holders,
                &[walled_order_revision],
                attack_order(vec![reference(&walled_target)], walled_policy),
            ),
        )
        .await;
    let (_, _, combat) = order_of(&outcome);
    assert!(
        combat.is_empty(),
        "a wall blocks the committed shot: {combat:?}"
    );
    assert_eq!(
        walled.health(walled_uuid).await,
        walled_health,
        "the hostile behind the wall keeps its health"
    );
    assert_eq!(
        arrow_count(&walled_storage, &walled_handle),
        4,
        "a blocked shot consumes no ammunition"
    );
}

#[tokio::test]
async fn changing_equipped_bow_to_sword_uses_melee_without_spending_an_arrow() {
    let fixture = Fixture::new(false);
    fixture.enable_arrows();
    let mut storage = fixture.storage();
    let (handle, resident_uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    seed_gear_in(
        &mut storage,
        &handle,
        resident_uuid,
        &[("minecraft:bow", 1), ("minecraft:arrow", 4)],
        true,
    );
    let zombie_id = fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(6.5, 64.0, 4.5)),
            true,
        )
        .expect("zombie is tracked");
    let zombie_uuid = fixture
        .sessions
        .snapshot_entity_for_test(zombie_id)
        .expect("zombie is live")
        .uuid;
    let handles = vec![handle.clone()];
    let revision = storage
        .resident_orders()
        .record(&handle)
        .unwrap()
        .order_revision();
    let hold = fixture
        .execute(
            &mut storage,
            &order_request(
                "weapon-hold",
                &handles,
                &[revision],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(4, 64, 4),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    let (_, members, _) = order_of(&hold);
    let target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hold issued a hostile reference")
        .clone();
    let before = fixture.health(zombie_uuid).await;
    seed_gear_in(
        &mut storage,
        &handle,
        resident_uuid,
        &[("minecraft:iron_sword", 1)],
        true,
    );
    let revision = storage
        .resident_orders()
        .record(&handle)
        .unwrap()
        .order_revision();
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "weapon-sword-attack",
                &handles,
                &[revision],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        target.target_ref,
                        target.policy_revision,
                        target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    let (_, members, combat) = order_of(&outcome);
    assert_eq!(combat.len(), 1, "the new sword commits a melee hit");
    assert!(fixture.health(zombie_uuid).await < before);
    assert_eq!(arrow_count(&storage, &handle), 4);
    assert!(
        fixture
            .sessions
            .resident_arrows_for_test(Vec3::new(4.5, 64.0, 4.5))
            .is_empty(),
        "the previous bow is not used after the weapon changes"
    );
    let target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("live target reference refreshed")
        .clone();
    seed_gear_in(
        &mut storage,
        &handle,
        resident_uuid,
        &[("minecraft:bow", 1)],
        true,
    );
    assert!(fixture.sessions.remove_tracked_entity_for_test(zombie_id));
    fixture.sessions.synchronize_entity_lifecycle_epoch(20);
    let revision = storage
        .resident_orders()
        .record(&handle)
        .unwrap()
        .order_revision();
    let departed = fixture
        .execute(
            &mut storage,
            &order_request(
                "weapon-departed-target",
                &handles,
                &[revision],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        target.target_ref,
                        target.policy_revision,
                        target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    let (_, _, combat) = order_of(&departed);
    assert!(combat.is_empty(), "departed target cannot be hit");
    assert_eq!(arrow_count(&storage, &handle), 4);
    assert!(
        fixture
            .sessions
            .resident_arrows_for_test(Vec3::new(4.5, 64.0, 4.5))
            .is_empty(),
        "the old reference cannot launch a projectile after the target leaves"
    );
}

/// A permitted distant target is chased, then attacked by the native owner
/// once movement reaches melee range; a guest does not issue per-tick damage.
#[tokio::test]
async fn distant_guard_engages_after_chasing_without_a_second_plugin_order() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, resident_uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let zombie_id = fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(9.5, 64.0, 4.5)),
            true,
        )
        .expect("zombie spawns");
    let zombie_uuid = fixture
        .sessions
        .snapshot_entity_for_test(zombie_id)
        .expect("zombie is live")
        .uuid;
    let hold = fixture
        .execute(
            &mut storage,
            &order_request(
                "distant-hold",
                std::slice::from_ref(&handle),
                &[0],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(4, 64, 4),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    let (_, members, _) = order_of(&hold);
    let target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("guard perceives the hostile")
        .clone();
    let attack = fixture
        .execute(
            &mut storage,
            &order_request(
                "distant-attack",
                std::slice::from_ref(&handle),
                &[members_revision(&hold)],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        target.target_ref,
                        target.policy_revision,
                        target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    let attack_revision = members_revision(&attack);
    assert_eq!(
        order_of(&attack).1[0].state,
        ScriptOrderMemberState::Applied,
        "accepted combat must not report a blocked formation route"
    );
    assert!(
        order_of(&attack).2.is_empty(),
        "out-of-reach order deals no damage"
    );
    let health = fixture.health(zombie_uuid).await;
    assert!(matches!(
        fixture.goal(resident_uuid).await,
        mc_entity::GoalState::FollowTarget { .. }
    ));
    let resident = fixture
        .sessions
        .resident_entity_snapshots(&[resident_uuid])
        .await
        .remove(0)
        .expect("guard stays live");
    fixture.sessions.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: resident.id,
            position: Vec3::new(7.5, 64.0, 4.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );
    assert!(fixture.position(resident_uuid).await.x >= 7.5);
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 1)
        .await
        .expect("native combat tick");
    let damaged = fixture.health(zombie_uuid).await;
    assert!(damaged < health, "native follow-up hits after approaching");
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 1)
        .await
        .expect("duplicate tick");
    assert_eq!(
        fixture.health(zombie_uuid).await,
        damaged,
        "cooldown gates repeat"
    );
    assert_eq!(
        storage
            .resident_orders()
            .record(&handle)
            .expect("guard record")
            .order_revision(),
        attack_revision,
        "native follow-up does not invalidate the guest's order fence"
    );
    let mut reopened = PluginStorage::open(fixture.storage_root.path()).expect("reopen combat");
    fixture
        .runtime
        .continue_active_resident_combat(&mut reopened, 21)
        .await
        .expect("cooldown elapsed after reopen");
    assert!(
        fixture.health(zombie_uuid).await < damaged,
        "the accepted chase and ammo/cooldown state survive reopening"
    );
    assert_eq!(
        reopened
            .resident_orders()
            .record(&handle)
            .expect("reopened guard")
            .order_revision(),
        attack_revision
    );
    let cancel = ScriptOperationRequest::try_new(
        "cancel",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::CancelOrder {
                operation_id: "distant-cancel".to_owned(),
                handles: vec![handle],
                expected_order_revisions: vec![attack_revision],
            },
        },
    )
    .expect("valid cancellation");
    let outcome = fixture.execute(&mut reopened, &cancel).await;
    assert_eq!(outcome.failure(), None, "accepted cancel: {outcome:?}");
    let health = fixture.health(zombie_uuid).await;
    fixture
        .runtime
        .continue_active_resident_combat(&mut reopened, 41)
        .await
        .expect("cancelled order no longer runs");
    assert_eq!(fixture.health(zombie_uuid).await, health);
}

/// A standing archer order spends its own canonical arrows on subsequent
/// native cooldown-ready shots, and stops launching when the stack runs out.
#[tokio::test]
async fn native_archer_follow_up_spends_each_arrow_once() {
    let fixture = Fixture::new(false);
    fixture.enable_arrows();
    let mut storage = fixture.storage();
    let (handle, archer) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let zombie_id = fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(10.5, 64.0, 4.5)),
            true,
        )
        .expect("zombie spawns");
    seed_gear(&mut storage, &handle, archer, &[("minecraft:bow", 1)]);
    let bow_revision = equip_bow(&mut storage, &handle);
    let hold = fixture
        .execute(
            &mut storage,
            &order_request(
                "native-bow-hold",
                std::slice::from_ref(&handle),
                &[bow_revision],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(4, 64, 4),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    let (_, members, _) = order_of(&hold);
    let target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile reference");
    seed_gear(&mut storage, &handle, archer, &[("minecraft:arrow", 2)]);
    let revision = storage
        .resident_orders()
        .record(&handle)
        .expect("archer record")
        .order_revision();
    let attack = fixture
        .execute(
            &mut storage,
            &order_request(
                "native-bow-attack",
                std::slice::from_ref(&handle),
                &[revision],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        target.target_ref.clone(),
                        target.policy_revision,
                        target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    let (_, _, combat) = order_of(&attack);
    assert!(combat.is_empty(), "launch is not impact");
    assert_eq!(arrow_count(&storage, &handle), 1);
    assert_eq!(
        fixture
            .sessions
            .resident_arrows_for_test(Vec3::new(4.5, 64.0, 4.5))
            .len(),
        1
    );
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 20)
        .await
        .expect("next cooldown shot");
    assert_eq!(arrow_count(&storage, &handle), 0);
    assert_eq!(
        fixture
            .sessions
            .resident_arrows_for_test(Vec3::new(4.5, 64.0, 4.5))
            .len(),
        2
    );
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 40)
        .await
        .expect("empty quiver does not launch");
    assert_eq!(
        fixture
            .sessions
            .resident_arrows_for_test(Vec3::new(4.5, 64.0, 4.5))
            .len(),
        2,
        "no ammunition yields no third world arrow"
    );
    assert!(
        fixture
            .sessions
            .snapshot_entity_for_test(zombie_id)
            .is_some(),
        "the target remains live until the shared arrow kernel resolves impact"
    );
}

/// A retreat whose rally cannot be placed must still stop the interrupted
/// chase and keep reporting the blocked route, instead of leaving the old
/// attack goal installed.
#[tokio::test]
async fn retreat_with_an_unplaceable_rally_stops_the_chase_and_reports_blocked() {
    let fixture = Fixture::new(false);
    fixture.seed_tree();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let handles = vec![handle.clone()];
    let zombie_id = fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(6.5, 64.0, 5.5)),
            true,
        )
        .expect("zombie spawns");
    let zombie_uuid = fixture
        .sessions
        .snapshot_entity_for_test(zombie_id)
        .expect("zombie is live")
        .uuid;

    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "blocked-retreat-1",
                &handles,
                &[0],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(4, 64, 4),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    let (_, members, _) = order_of(&outcome);
    let hostile_target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile perceived")
        .clone();
    let order_revision = members_revision(&outcome);
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "blocked-retreat-2",
                &handles,
                &[order_revision],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        hostile_target.target_ref.clone(),
                        hostile_target.policy_revision,
                        hostile_target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    let attack_revision = members_revision(&outcome);
    assert!(
        matches!(
            fixture.goal(uuid).await,
            mc_entity::GoalState::FollowTarget { .. }
        ),
        "attack chases the hostile"
    );
    // The fixture's bounded tree occupies this cell, so its formation slot
    // cannot be placed.
    assert_eq!(
        fixture.block(6, SURFACE_Y + 1, 6).await,
        Some(state_of(&fixture.blocks, "oak_log")),
        "the blocked rally cell is occupied"
    );

    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "blocked-retreat-3",
                &handles,
                &[attack_revision],
                ScriptResidentOrder::Retreat {
                    anchor: ScriptBlockPosition::new(6, SURFACE_Y + 1, 6),
                    formation: formation(),
                },
            ),
        )
        .await;
    let (_, members, _) = order_of(&outcome);
    assert_eq!(
        members[0].state,
        ScriptOrderMemberState::BlockedRoute,
        "an unplaceable rally reports blocked_route"
    );
    assert_eq!(
        fixture.goal(uuid).await,
        mc_entity::GoalState::Idle,
        "the blocked retreat stops the interrupted chase"
    );
    let health_after_retreat = fixture.health(zombie_uuid).await;
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 100)
        .await
        .expect("retreat leaves no attack continuation");
    assert_eq!(
        fixture.health(zombie_uuid).await,
        health_after_retreat,
        "the prior chase cannot land another hit after a blocked retreat"
    );
}

/// (A11) Retreat interrupts the attack chase and patrol restores the route
/// after the threat is gone.
#[tokio::test]
async fn retreat_interrupts_attack_and_patrol_walks_its_waypoints() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let handles = vec![handle.clone()];
    let zombie_id = fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(6.5, 64.0, 4.5)),
            true,
        )
        .expect("zombie spawns");
    let zombie_uuid = fixture
        .sessions
        .snapshot_entity_for_test(zombie_id)
        .expect("zombie is live")
        .uuid;

    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "stage-1",
                &handles,
                &[0],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(4, 64, 4),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    let (_, members, _) = order_of(&outcome);
    let hostile_target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile perceived")
        .clone();
    let order_revision = members_revision(&outcome);
    let hostile_health = fixture.health(zombie_uuid).await;

    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "stage-2",
                &handles,
                &[order_revision],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        hostile_target.target_ref.clone(),
                        hostile_target.policy_revision,
                        hostile_target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    let (_, _, combat) = order_of(&outcome);
    assert_eq!(
        combat.len(),
        1,
        "the melee action committed exactly one hit"
    );
    assert_eq!(combat[0].attacker_handle, handle);
    assert!(combat[0].damage_milli > 0);
    assert!(fixture.health(zombie_uuid).await < hostile_health);
    let attack_revision = members_revision(&outcome);
    assert!(
        matches!(
            fixture.goal(uuid).await,
            mc_entity::GoalState::FollowTarget { .. }
        ),
        "attack chases the hostile"
    );

    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "stage-3",
                &handles,
                &[attack_revision],
                ScriptResidentOrder::Retreat {
                    anchor: ScriptBlockPosition::new(2, 64, 4),
                    formation: formation(),
                },
            ),
        )
        .await;
    let (_, _, _) = order_of(&outcome);
    let retreat_revision = members_revision(&outcome);
    assert!(
        matches!(
            fixture.goal(uuid).await,
            mc_entity::GoalState::FollowPosition { .. }
        ),
        "retreat replaces the chase"
    );
    assert_eq!(
        fixture.position(uuid).await.x,
        4.5,
        "retreat never teleports the member"
    );
    let health_after_retreat = fixture.health(zombie_uuid).await;
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 100)
        .await
        .expect("retreat leaves no attack continuation");
    assert_eq!(
        fixture.health(zombie_uuid).await,
        health_after_retreat,
        "the prior chase cannot land another hit after retreat"
    );

    let waypoints = vec![
        ScriptBlockPosition::new(3, 64, 3),
        ScriptBlockPosition::new(3, 64, 5),
        ScriptBlockPosition::new(5, 64, 5),
    ];
    let patrol = ScriptResidentOrder::Patrol {
        waypoints: waypoints.clone(),
        formation: formation(),
        engagement_radius: 8,
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request("stage-4", &handles, &[retreat_revision], patrol.clone()),
        )
        .await;
    let (_, _, _) = order_of(&outcome);
    let first_leg = fixture.goal(uuid).await;
    assert!(
        matches!(first_leg, mc_entity::GoalState::FollowPosition { .. }),
        "patrol walks a waypoint: {first_leg:?}"
    );
    let record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("patrol record");
    let first_index = record.order.as_ref().expect("patrol order").route_index;
    let patrol_revision = members_revision(&outcome);
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request("stage-5", &handles, &[patrol_revision], patrol.clone()),
        )
        .await;
    let (_, members, _) = order_of(&outcome);
    let record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("patrol record");
    assert_ne!(
        record.order.as_ref().expect("patrol order").route_index,
        first_index,
        "the route advances to the next waypoint"
    );
    let advanced_revision = members_revision(&outcome);
    let hostile_target = members[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("patrol refreshed hostile reference")
        .clone();

    // A threat interrupts the route: the member chases instead of walking.
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request(
                "stage-6",
                &handles,
                &[advanced_revision],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        hostile_target.target_ref,
                        hostile_target.policy_revision,
                        hostile_target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(2, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    let (_, _, _) = order_of(&outcome);
    let threat_leg = fixture.goal(uuid).await;
    assert!(
        matches!(threat_leg, mc_entity::GoalState::FollowTarget { .. }),
        "the threat interrupts the patrol route: {threat_leg:?}"
    );

    // Once the threat is gone, the same patrol order resumes the route.
    let threat_revision = members_revision(&outcome);
    let outcome = fixture
        .execute(
            &mut storage,
            &order_request("stage-7", &handles, &[threat_revision], patrol),
        )
        .await;
    let (_, _, _) = order_of(&outcome);
    let resumed_leg = fixture.goal(uuid).await;
    assert!(
        matches!(resumed_leg, mc_entity::GoalState::FollowPosition { .. }),
        "the patrol returns to its route: {resumed_leg:?}"
    );
    let record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("patrol record");
    assert!(
        matches!(
            record.order.as_ref().expect("patrol order").order,
            ScriptResidentOrder::Patrol { .. }
        ),
        "the resumed order is the patrol route"
    );
}

#[tokio::test]
async fn injury_and_flank_route_then_capture_the_same_resident() {
    let fixture = Fixture::new(false);
    use crate::play::ResidentAttack;
    use crate::script::storage::resident_morale::MoralePhase;

    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    seed_gear(&mut storage, &handle, uuid, &[("minecraft:emerald", 1)]);
    let zombie_id = fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(6.5, 64.0, 4.5)),
            true,
        )
        .expect("hostile spawns");
    let zombie_uuid = fixture
        .sessions
        .snapshot_entity_for_test(zombie_id)
        .expect("hostile exists")
        .uuid;
    let hold = fixture
        .execute(
            &mut storage,
            &order_request(
                "morale-hold",
                std::slice::from_ref(&handle),
                &[0],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(4, 64, 4),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    let target = order_of(&hold).1[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile is perceived")
        .clone();
    let attack = fixture
        .execute(
            &mut storage,
            &order_request(
                "morale-attack",
                std::slice::from_ref(&handle),
                &[members_revision(&hold)],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        target.target_ref,
                        target.policy_revision,
                        target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            Vec::new(),
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.rally = Some(ScriptBlockPosition::new(1, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    assert_eq!(attack.failure(), None);
    let public_order_revision = members_revision(&attack);
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 100)
        .await
        .unwrap();
    assert_eq!(
        storage
            .resident_orders()
            .record(&handle)
            .unwrap()
            .morale
            .as_ref()
            .unwrap()
            .phase,
        MoralePhase::Shaken,
        "the hostile is physically on the resident's flank"
    );
    for (tick, expected) in [(101, MoralePhase::Wavering), (102, MoralePhase::Routing)] {
        let snapshot = fixture.sessions.resident_entity_snapshots(&[uuid]).await[0]
            .clone()
            .expect("resident remains native and alive");
        let hit = fixture
            .sessions
            .commit_resident_damage(
                OWNER,
                vec![ResidentAttack {
                    uuid,
                    amount: 1.0,
                    expected: snapshot,
                }],
            )
            .await;
        assert!(
            hit[0].is_some(),
            "each injury must be a committed native hit"
        );
        fixture
            .runtime
            .continue_active_resident_combat(&mut storage, tick)
            .await
            .unwrap();
        assert_eq!(
            storage
                .resident_orders()
                .record(&handle)
                .unwrap()
                .morale
                .as_ref()
                .unwrap()
                .phase,
            expected
        );
    }
    assert_eq!(fixture.position(uuid).await, Vec3::new(4.5, 64.0, 4.5));
    assert!(matches!(
        fixture.goal(uuid).await,
        mc_entity::GoalState::FollowPosition { target, .. }
            if target == Vec3::new(1.5, 64.0, 4.5)
    ));
    let hostile_health = fixture.health(zombie_uuid).await;
    let mut reopened = PluginStorage::open(fixture.storage_root.path()).unwrap();
    fixture
        .runtime
        .recover_resident_orders(&mut reopened)
        .await
        .unwrap();
    fixture
        .runtime
        .continue_active_resident_combat(&mut reopened, 200)
        .await
        .unwrap();
    assert_eq!(fixture.health(zombie_uuid).await, hostile_health);
    assert_eq!(
        reopened
            .resident_orders()
            .record(&handle)
            .unwrap()
            .morale
            .as_ref()
            .unwrap()
            .phase,
        MoralePhase::Routing,
        "reload cannot silently restore the interrupted chase"
    );
    let (distant_guard, _) = fixture
        .resident(&mut reopened, 4, Vec3::new(11.5, 64.0, 4.5))
        .await;
    let distant_hold = fixture
        .execute(
            &mut reopened,
            &order_request(
                "distant-guard-hold",
                std::slice::from_ref(&distant_guard),
                &[0],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(11, 64, 4),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    assert_eq!(distant_hold.failure(), None);
    let revision = public_order_revision;
    let internal_revision = reopened.resident_orders().record(&handle).unwrap().revision;
    assert!(
        internal_revision > revision,
        "morale and gear progress must not fence off the public attack order"
    );
    let distant = fixture
        .execute(
            &mut reopened,
            &capture_request("capture-too-far", &handle, &distant_guard, revision),
        )
        .await;
    assert_eq!(
        distant.failure(),
        Some(ScriptOperationFailure::InvalidRequest)
    );
    assert_eq!(
        reopened.resident_orders().record(&handle).unwrap().revision,
        internal_revision
    );

    let (converted_guard, converted_uuid) = fixture
        .resident(&mut reopened, 6, Vec3::new(5.5, 64.0, 4.5))
        .await;
    let converted_hold = fixture
        .execute(
            &mut reopened,
            &order_request(
                "converted-guard-hold",
                std::slice::from_ref(&converted_guard),
                &[0],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(5, 64, 4),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    assert_eq!(converted_hold.failure(), None);
    let converted_id = fixture
        .sessions
        .resident_entity_snapshots(&[converted_uuid])
        .await[0]
        .as_ref()
        .expect("guard is originally a villager")
        .id;
    assert!(
        fixture
            .sessions
            .convert_resident_entity_for_test(converted_id)
    );
    let converted_capture = fixture
        .execute(
            &mut reopened,
            &capture_request(
                "capture-converted-guard",
                &handle,
                &converted_guard,
                revision,
            ),
        )
        .await;
    assert_eq!(
        converted_capture.failure(),
        Some(ScriptOperationFailure::NotFound),
        "a former villager converted to a zombie cannot claim custody"
    );

    let (guard, _) = fixture
        .resident(&mut reopened, 5, Vec3::new(5.5, 64.0, 5.5))
        .await;
    let nearby_hold = fixture
        .execute(
            &mut reopened,
            &order_request(
                "nearby-guard-hold",
                std::slice::from_ref(&guard),
                &[0],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(5, 64, 5),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    assert_eq!(nearby_hold.failure(), None);
    let before_gear = gear(&reopened, &handle);
    let capture = capture_request("capture-nearby", &handle, &guard, revision);
    let captured = fixture.execute(&mut reopened, &capture).await;
    let ScriptOperationPayload::ResidentOrder { result } = captured.payload() else {
        panic!("capture must return the original identity");
    };
    assert!(matches!(&**result,
        ScriptResidentOrderResult::Captured { handle: victim, custodian, .. }
            if victim == &handle && custodian == &guard
    ));
    let prisoner = reopened.resident_orders().record(&handle).unwrap();
    assert_eq!(
        prisoner.assignment,
        crate::script::storage::resident_orders::DurableAssignment::Prisoner
    );
    assert_eq!(prisoner.entity_uuid, uuid.to_string());
    assert_eq!(prisoner.custodian.as_deref(), Some(guard.as_str()));
    assert_eq!(
        prisoner.morale.as_ref().unwrap().phase,
        MoralePhase::Surrendered
    );
    assert_eq!(gear(&reopened, &handle), before_gear);
    assert_eq!(fixture.goal(uuid).await, mc_entity::GoalState::Idle);
    assert_eq!(fixture.position(uuid).await, Vec3::new(4.5, 64.0, 4.5));
    let repeat = fixture.execute(&mut reopened, &capture).await;
    assert_eq!(
        repeat, captured,
        "same native decision is replayed exactly once"
    );
    let mut restored = PluginStorage::open(fixture.storage_root.path()).unwrap();
    fixture
        .runtime
        .recover_resident_orders(&mut restored)
        .await
        .unwrap();
    assert_eq!(
        restored
            .resident_orders()
            .record(&handle)
            .unwrap()
            .entity_uuid,
        uuid.to_string()
    );
    assert_eq!(gear(&restored, &handle), before_gear);
    assert_eq!(
        fixture
            .execute(
                &mut restored,
                &demobilize_request("prisoner-demob", &handle, captured.revision().unwrap()),
            )
            .await
            .failure(),
        Some(ScriptOperationFailure::NotFound),
        "a captive cannot be enrolled or demobilized as a second army member"
    );
}

/// (CP-047 / WAR-07) A real ally death and a real officer death each advance one
/// morale category past the physical flank, and the routed squad receives its
/// rally as the native goal instead of the interrupted chase.
#[tokio::test]
async fn physical_ally_and_officer_losses_route_the_squad_to_its_rally() {
    let fixture = Fixture::new(false);
    use crate::play::ResidentAttack;
    use crate::script::storage::resident_morale::MoralePhase;

    let mut storage = fixture.storage();
    let (guard, guard_uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let (ally, ally_uuid) = fixture
        .resident(&mut storage, 4, Vec3::new(5.5, 64.0, 4.5))
        .await;
    let (officer, officer_uuid) = fixture
        .resident(&mut storage, 5, Vec3::new(5.5, 64.0, 5.5))
        .await;
    for (operation, member) in [("ally-post", &ally), ("officer-post", &officer)] {
        let post = fixture
            .execute(
                &mut storage,
                &order_request(
                    operation,
                    std::slice::from_ref(member),
                    &[0],
                    ScriptResidentOrder::Hold {
                        anchor: ScriptBlockPosition::new(5, 64, 5),
                        heading_degrees: 0,
                        formation: formation(),
                        engagement_radius: 16,
                    },
                ),
            )
            .await;
        assert_eq!(post.failure(), None, "{operation}: {post:?}");
    }
    fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(6.5, 64.0, 4.5)),
            true,
        )
        .expect("hostile spawns");
    let hold = fixture
        .execute(
            &mut storage,
            &order_request(
                "loss-hold",
                std::slice::from_ref(&guard),
                &[0],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(4, 64, 4),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    let target = order_of(&hold).1[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile is perceived")
        .clone();
    let attack = fixture
        .execute(
            &mut storage,
            &order_request(
                "loss-attack",
                std::slice::from_ref(&guard),
                &[members_revision(&hold)],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        target.target_ref,
                        target.policy_revision,
                        target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            vec![guard.clone(), ally.clone(), officer.clone()],
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.officer = Some(officer.clone());
                        policy.rally = Some(ScriptBlockPosition::new(1, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    assert_eq!(attack.failure(), None, "{attack:?}");
    let phase = |storage: &PluginStorage| {
        storage
            .resident_orders()
            .record(&guard)
            .expect("the guard keeps its military record")
            .morale
            .as_ref()
            .expect("the attack order carries morale")
            .phase
    };
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 100)
        .await
        .unwrap();
    assert_eq!(
        phase(&storage),
        MoralePhase::Shaken,
        "the hostile stands on the guard's flank"
    );

    let expected = fixture
        .sessions
        .resident_entity_snapshots(&[ally_uuid])
        .await[0]
        .clone()
        .expect("the ally is native and alive");
    let hit = fixture
        .sessions
        .commit_resident_damage(
            OWNER,
            vec![ResidentAttack {
                uuid: ally_uuid,
                amount: 1_000.0,
                expected,
            }],
        )
        .await;
    assert!(
        hit[0].as_ref().is_some_and(|hit| hit.killed),
        "the ally must really die: {hit:?}"
    );
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 101)
        .await
        .unwrap();
    assert_eq!(
        phase(&storage),
        MoralePhase::Wavering,
        "one perceived ally loss is one category"
    );

    let expected = fixture
        .sessions
        .resident_entity_snapshots(&[officer_uuid])
        .await[0]
        .clone()
        .expect("the officer is native and alive");
    let hit = fixture
        .sessions
        .commit_resident_damage(
            OWNER,
            vec![ResidentAttack {
                uuid: officer_uuid,
                amount: 1_000.0,
                expected,
            }],
        )
        .await;
    assert!(
        hit[0].as_ref().is_some_and(|hit| hit.killed),
        "the officer must really die: {hit:?}"
    );
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 102)
        .await
        .unwrap();
    assert_eq!(
        phase(&storage),
        MoralePhase::Routing,
        "the officer's real death ends morale"
    );
    assert!(
        matches!(
            fixture.goal(guard_uuid).await,
            mc_entity::GoalState::FollowPosition { target, .. }
                if target == Vec3::new(1.5, 64.0, 4.5)
        ),
        "routing replaces the chase with the squad's rally"
    );
    let reopened = PluginStorage::open(fixture.storage_root.path()).expect("reopen morale");
    assert_eq!(
        reopened
            .resident_orders()
            .record(&guard)
            .unwrap()
            .morale
            .as_ref()
            .unwrap()
            .phase,
        MoralePhase::Routing,
        "the routed squad survives a restart"
    );
}

/// (CP-047 / WAR-07) A routed squad that physically walks to its rally with its
/// officer attending is restored: arrival, not elapsed time, ends the rout.
#[tokio::test]
async fn physical_rally_arrival_restores_the_routed_squad() {
    let fixture = Fixture::new(false);
    use crate::play::ResidentAttack;
    use crate::script::storage::resident_morale::MoralePhase;
    use mc_physics::{BlockMaterial, BlockMaterialIds, BlockSampler, EntityBody, PhysicsConfig};

    struct Terrain<'a> {
        chunks: &'a mc_world::WorldReadSnapshot,
        materials: &'a BlockMaterialIds,
    }
    impl BlockSampler for Terrain<'_> {
        fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
            self.chunks
                .get_cached_block(BlockPos { x, y, z })
                .map_or(BlockMaterial::Air, |state| self.materials.classify(state.0))
        }
    }
    let mut storage = fixture.storage();
    let (guard, guard_uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(4.5, 64.0, 4.5))
        .await;
    let (officer, _) = fixture
        .resident(&mut storage, 5, Vec3::new(5.5, 64.0, 5.5))
        .await;
    let post = fixture
        .execute(
            &mut storage,
            &order_request(
                "rally-officer-post",
                std::slice::from_ref(&officer),
                &[0],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(5, 64, 5),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    assert_eq!(post.failure(), None, "{post:?}");
    fixture
        .sessions
        .spawn_tracked_entity_for_test(
            SpawnEntity::new(54, "minecraft:zombie", Vec3::new(6.5, 64.0, 4.5)),
            true,
        )
        .expect("hostile spawns");
    let hold = fixture
        .execute(
            &mut storage,
            &order_request(
                "rally-hold",
                std::slice::from_ref(&guard),
                &[0],
                ScriptResidentOrder::Hold {
                    anchor: ScriptBlockPosition::new(4, 64, 4),
                    heading_degrees: 0,
                    formation: formation(),
                    engagement_radius: 16,
                },
            ),
        )
        .await;
    let target = order_of(&hold).1[0]
        .targets
        .iter()
        .find(|target| target.category == ScriptHostileCategory::Hostile)
        .expect("hostile is perceived")
        .clone();
    let attack = fixture
        .execute(
            &mut storage,
            &order_request(
                "rally-attack",
                std::slice::from_ref(&guard),
                &[members_revision(&hold)],
                ScriptResidentOrder::Attack {
                    targets: vec![ScriptOrderTargetRef::new(
                        target.target_ref,
                        target.policy_revision,
                        target.expires_revision,
                    )],
                    policy: {
                        let mut policy = ScriptEngagementPolicy::new(
                            0,
                            vec![guard.clone(), officer.clone()],
                            vec![ScriptHostileCategory::Hostile],
                        );
                        policy.officer = Some(officer.clone());
                        policy.rally = Some(ScriptBlockPosition::new(1, 64, 4));
                        policy
                    },
                },
            ),
        )
        .await;
    assert_eq!(attack.failure(), None, "{attack:?}");
    let phase = |storage: &PluginStorage| {
        storage
            .resident_orders()
            .record(&guard)
            .expect("the guard keeps its military record")
            .morale
            .as_ref()
            .expect("the attack order carries morale")
            .phase
    };
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 100)
        .await
        .unwrap();
    assert_eq!(phase(&storage), MoralePhase::Shaken);
    let mut before_routing = None;
    for tick in [101, 102] {
        let expected = fixture
            .sessions
            .resident_entity_snapshots(&[guard_uuid])
            .await[0]
            .clone()
            .expect("the guard stays native and alive");
        let hit = fixture
            .sessions
            .commit_resident_damage(
                OWNER,
                vec![ResidentAttack {
                    uuid: guard_uuid,
                    amount: 1.0,
                    expected,
                }],
            )
            .await;
        assert!(hit[0].is_some(), "each injury is a committed native hit");
        if tick == 102 {
            before_routing = Some(fixture.position(guard_uuid).await);
        }
        fixture
            .runtime
            .continue_active_resident_combat(&mut storage, tick)
            .await
            .unwrap();
    }
    assert_eq!(phase(&storage), MoralePhase::Routing);
    assert!(
        matches!(
            fixture.goal(guard_uuid).await,
            mc_entity::GoalState::FollowPosition { target, .. }
                if target == Vec3::new(1.5, 64.0, 4.5)
        ),
        "the rout walks the squad to its rally instead of chasing"
    );
    // Routing replaces the goal; it must not move the guard itself, and the
    // guard must still start outside the rally's two-block arrival radius.
    let before_routing = before_routing.expect("the routing tick captured the guard's position");
    let routed_at = fixture.position(guard_uuid).await;
    assert!(
        (routed_at.x - before_routing.x).abs() < 0.001
            && (routed_at.z - before_routing.z).abs() < 0.001,
        "routing must not move the guard: {before_routing:?} -> {routed_at:?}"
    );
    assert!(
        (routed_at.x - 1.5).powi(2) + (routed_at.z - 4.5).powi(2) > 4.0,
        "the guard must walk to its rally instead of starting inside the arrival radius: {routed_at:?}"
    );

    // The squad reaches the rally by its own native walking goal.
    fixture
        .sessions
        .register_loaded_for_server_test("RallyObserver", (0, 0));
    let world_read = fixture.world.lock().await.read_view();
    let materials = BlockMaterialIds::new(
        state_of(&fixture.blocks, "air").0,
        Some(state_of(&fixture.blocks, "water").0),
        None,
    );
    let chunks = world_read.snapshot_chunks(&[ChunkPos { x: 0, z: 0 }]);
    let terrain = Terrain {
        chunks: &chunks,
        materials: &materials,
    };
    let guard_id = fixture
        .sessions
        .resident_entity_snapshots(&[guard_uuid])
        .await[0]
        .as_ref()
        .expect("living guard")
        .id;
    let mut walked_from = routed_at;
    for tick in 200..=400 {
        let queries = fixture
            .sessions
            .tick_entities_and_collect_physics_queries_with_terrain(tick, &world_read, &materials);
        let steps = queries
            .iter()
            .filter(|query| query.id == guard_id)
            .map(|query| {
                let stepped = mc_physics::step_entity(
                    EntityBody {
                        position: mc_physics::Vec3::new(
                            query.position.x,
                            query.position.y,
                            query.position.z,
                        ),
                        velocity: mc_physics::Vec3::new(
                            query.velocity.x,
                            query.velocity.y,
                            query.velocity.z,
                        ),
                        aabb: query.aabb,
                        on_ground: query.on_ground,
                    },
                    &terrain,
                    PhysicsConfig::living_entity(),
                );
                EntityPhysicsStep {
                    id: query.id,
                    position: Vec3::new(
                        stepped.body.position.x,
                        stepped.body.position.y,
                        stepped.body.position.z,
                    ),
                    velocity: Vec3::new(
                        stepped.body.velocity.x,
                        stepped.body.velocity.y,
                        stepped.body.velocity.z,
                    ),
                    on_ground: stepped.body.on_ground,
                    horizontal_collision: stepped.horizontal_collision,
                }
            })
            .collect::<Vec<_>>();
        fixture
            .sessions
            .apply_entity_physics_if_current_and_dispatch(tick, &queries, &steps);
        let at = fixture.position(guard_uuid).await;
        let step = ((at.x - walked_from.x).powi(2) + (at.z - walked_from.z).powi(2)).sqrt();
        assert!(
            step < 1.0,
            "one native physics tick must not teleport the guard: {walked_from:?} -> {at:?}"
        );
        walked_from = at;
        if (at.x - 1.5).abs() < 0.5 && (at.z - 4.5).abs() < 0.5 {
            break;
        }
    }
    let arrived = fixture.position(guard_uuid).await;
    assert!(
        (arrived.x - 1.5).abs() < 0.5 && (arrived.z - 4.5).abs() < 0.5,
        "the routed guard must walk to its rally, not teleport: {arrived:?}"
    );
    fixture
        .runtime
        .continue_active_resident_combat(&mut storage, 401)
        .await
        .unwrap();
    assert_eq!(
        phase(&storage),
        MoralePhase::Rallied,
        "the officer at the rally restores the squad"
    );
    assert_eq!(fixture.goal(guard_uuid).await, mc_entity::GoalState::Idle);
    let reopened = PluginStorage::open(fixture.storage_root.path()).expect("reopen morale");
    let record = reopened.resident_orders().record(&guard).unwrap().clone();
    let morale = record.morale.as_ref().expect("terminal morale");
    assert_eq!(morale.phase, MoralePhase::Rallied);
    assert!(
        morale.goal_applied,
        "the restored squad's goal is durably applied"
    );
}
/// Equip the seeded bow through the durable order ledger. A record write
/// re-stamps the nested order revision, so the write's own revision is the
/// fence the next order must send.
fn equip_bow(storage: &mut PluginStorage, handle: &str) -> u64 {
    let mut record = storage
        .resident_orders()
        .record(handle)
        .cloned()
        .expect("resident record");
    let held = record.carry[0].take().expect("bow stack");
    record.equipment[0] = Some(held);
    storage
        .append_resident_order_change(DurableResidentOrderChange::Record {
            record: Box::new(record),
        })
        .unwrap();
    storage
        .resident_orders()
        .record(handle)
        .expect("resident record")
        .order_revision()
}

fn arrow_count(storage: &PluginStorage, handle: &str) -> u32 {
    gear(storage, handle)
        .iter()
        .filter(|(item, _)| item == "minecraft:arrow")
        .map(|(_, count)| *count)
        .sum()
}

fn capture_request(
    operation_id: &str,
    handle: &str,
    custodian: &str,
    expected_revision: u64,
) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::Capture {
                operation_id: operation_id.to_owned(),
                handle: handle.to_owned(),
                custodian: custodian.to_owned(),
                expected_revision,
            },
        },
    )
    .expect("valid capture request")
}
