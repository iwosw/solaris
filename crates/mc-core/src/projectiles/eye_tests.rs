use super::flight_tests::setup;
use crate::boundary::Hand;
use mc_data::{Identifier, ItemStack};

#[test]
fn physics_eye_without_stronghold_is_handled_without_consumption() {
    let (mut state, player, _) = setup();
    let item = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:ender_eye").unwrap())
        .unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] = ItemStack::new(item, 2);
    assert!(super::weapons::use_item(&mut state, player, Hand::Main));
    assert_eq!(state.players[&player].inventory.slots[0].count, 2);
    assert!(state.projectiles.spawned.is_empty());
}

fn request(state: &crate::tick::TickState, actor: crate::boundary::EntityId) -> super::eye::EyeUse {
    let player = &state.players[&actor];
    let mut stack = player.inventory.slots[0].clone();
    stack.count = 1;
    super::eye::EyeUse {
        actor,
        connection: player.connection,
        hand: Hand::Main,
        origin: super::weapons::geometry(state, actor, 0.0).unwrap().0,
        dimension: state.dimension,
        stack,
    }
}

#[test]
fn physics_eye_completion_rechecks_custody_and_creative_consumption() {
    for creative in [false, true] {
        let (mut state, player, _) = setup();
        let item = state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:ender_eye").unwrap())
            .unwrap();
        state.players.get_mut(&player).unwrap().inventory.slots[0] = ItemStack::new(item, 2);
        if creative {
            state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Creative;
        }
        let pending = request(&state, player);
        let origin = pending.origin;
        super::eye::complete(
            &mut state,
            pending,
            (origin[0] as i32 + 100, 40, origin[2] as i32),
        );
        assert_eq!(
            state.players[&player].inventory.slots[0].count,
            if creative { 2 } else { 1 }
        );
        assert_eq!(state.projectiles.spawned.len(), 1);
        let id = state.projectiles.spawned[0];
        let slot = state.entities.slot_of(id).unwrap();
        let record = state.entities.row(slot).projectile.as_ref().unwrap();
        assert_eq!(record.kind, super::ProjectileKind::EyeOfEnder);
        assert_eq!(state.entities.row(slot).appearance.spawn_data, 0);
        assert!(state.entities.row(slot).appearance.extra.iter().any(|(index, value)|
            *index == 8 && matches!(value, crate::boundary::EntityDataValue::Item(Some(stack)) if stack.item_id == item && stack.count == 1)));
        let bytes = serde_json::to_vec(record).unwrap();
        let restored: super::ProjectileState = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(&restored, record.as_ref());
        assert!(super::persistence::validate_state(&restored).is_ok());
        let pending = request(&state, player);
        state.players.get_mut(&player).unwrap().inventory.slots[0] = ItemStack::EMPTY;
        super::eye::complete(&mut state, pending, (100, 40, 100));
        assert_eq!(state.projectiles.spawned.len(), 1);
    }
}

#[test]
fn physics_eye_rises_twelve_blocks_hovers_and_expires_once() {
    for drop_item in [false, true] {
        let (mut state, player, _) = setup();
        let item = state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:ender_eye").unwrap())
            .unwrap();
        state.players.get_mut(&player).unwrap().inventory.slots[0] = ItemStack::new(item, 2);
        let pending = request(&state, player);
        let origin = pending.origin;
        super::eye::complete(
            &mut state,
            pending,
            (origin[0] as i32 + 100, 40, origin[2] as i32),
        );
        let id = state.projectiles.spawned[0];
        let slot = state.entities.slot_of(id).unwrap();
        let record = state.entities.row_mut(slot).projectile.as_mut().unwrap();
        let super::ProjectileData::Eye {
            drop_item: drops, ..
        } = &mut record.data
        else {
            panic!("eye data")
        };
        *drops = drop_item;
        crate::tick::run_tick(&mut state, &[]);
        assert_eq!(
            state
                .entities
                .row(state.entities.slot_of(id).unwrap())
                .projectile
                .as_ref()
                .unwrap()
                .age,
            1
        );
        for _ in 1..40 {
            super::flight::run(&mut state, &mut Vec::new(), std::iter::once(id));
        }
        let at = state.entities.pos(state.entities.slot_of(id).unwrap());
        let dx = at[0] - origin[0];
        let dz = at[2] - origin[2];
        assert!(((dx * dx + dz * dz).sqrt() - 12.0).abs() < 1e-6);
        assert_eq!(at[1], origin[1] + 8.0);
        for _ in 0..40 {
            super::flight::run(&mut state, &mut Vec::new(), std::iter::once(id));
        }
        assert_eq!(state.entities.pos(state.entities.slot_of(id).unwrap()), at);
        let before = state.projectiles.spawned.len();
        super::flight::run(&mut state, &mut Vec::new(), std::iter::once(id));
        assert!(state.entities.slot_of(id).is_none());
        assert_eq!(
            state.projectiles.spawned.len() - before,
            usize::from(drop_item)
        );
        super::flight::run(&mut state, &mut Vec::new(), std::iter::once(id));
        assert_eq!(
            state.projectiles.spawned.len() - before,
            usize::from(drop_item)
        );
    }
}

#[test]
fn physics_eye_corrupt_retained_target_fails_closed() {
    let mut record =
        super::ProjectileState::new(super::ProjectileKind::EyeOfEnder, None, ItemStack::EMPTY);
    record.data = super::ProjectileData::Eye {
        origin: [0.0; 3],
        target: [f64::NAN, 0.0, 0.0],
        drop_item: false,
    };
    assert!(super::persistence::validate_state(&record).is_err());
    record.kind = super::ProjectileKind::Arrow;
    record.data = super::ProjectileData::Eye {
        origin: [0.0; 3],
        target: [0.0; 3],
        drop_item: false,
    };
    assert!(super::persistence::validate_state(&record).is_err());
}
