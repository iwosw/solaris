use super::super::{flight_tests::setup, launch};
use super::*;
use crate::boundary::EntityAppearance;
use crate::survival::DamageKind;
use crate::tick::{DamagePiece, run_tick};
use mc_data::{Identifier, ItemStack};

fn target(state: &mut TickState, centre: [f64; 3]) -> EntityId {
    let cow = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let id = state.spawn_mob(cow, [centre[0] + 2.0, centre[1], centre[2]], 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).controls.no_ai = true;
    state.entities.row_mut(slot).speed = 0.0;
    id
}

fn shot(
    state: &mut TickState,
    player: EntityId,
    centre: [f64; 3],
    kind: ProjectileKind,
    piercing: u32,
) {
    let owner = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    let item = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:arrow").unwrap())
        .unwrap();
    let mut record = ProjectileState::new(kind, Some(owner), ItemStack::new(item, 1));
    record.damage = 2.0;
    record.piercing = piercing;
    launch(
        state,
        record,
        [centre[0], centre[1] + 0.7, centre[2]],
        [3.0, 0.0, 0.0],
    )
    .unwrap();
    run_tick(state, &[]);
}

#[test]
fn embedded_projectiles_accepted_arrow_and_spectral_hits_increment_once() {
    for kind in [ProjectileKind::Arrow, ProjectileKind::SpectralArrow] {
        let (mut state, player, centre) = setup();
        let victim = target(&mut state, centre);
        shot(&mut state, player, centre, kind, 0);
        let slot = state.entities.slot_of(victim).unwrap();
        assert!(state.entities.row(slot).health < 20.0);
        assert_eq!(state.entities.row(slot).appearance.arrow_count, 1);
        assert!(state.projectiles.embedded_hits.is_empty());
        run_tick(&mut state, &[]);
        assert_eq!(
            state
                .entities
                .row(state.entities.slot_of(victim).unwrap())
                .appearance
                .arrow_count,
            1
        );
    }
}

#[test]
fn embedded_projectiles_piercing_trident_and_refused_hits_do_not_embed() {
    for (kind, piercing, refused) in [
        (ProjectileKind::Arrow, 1, false),
        (ProjectileKind::Trident, 0, false),
        (ProjectileKind::Arrow, 0, true),
    ] {
        let (mut state, player, centre) = setup();
        let victim = target(&mut state, centre);
        let slot = state.entities.slot_of(victim).unwrap();
        state.entities.row_mut(slot).controls.invulnerable = refused;
        shot(&mut state, player, centre, kind, piercing);
        let slot = state.entities.slot_of(victim).unwrap();
        assert_eq!(state.entities.row(slot).appearance.arrow_count, 0);
        assert_eq!(state.entities.row(slot).health == 20.0, refused);
        run_tick(&mut state, &[]);
        assert!(state.projectiles.embedded_hits.is_empty());
    }
}

#[test]
fn embedded_projectiles_bee_accepted_sting_embeds_once() {
    let (mut state, _, centre) = setup();
    let victim = target(&mut state, centre);
    let bee_type = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:bee").unwrap())
        .unwrap();
    let bee = state.spawn_mob(bee_type, centre, 10.0);
    let bee_slot = state.entities.slot_of(bee).unwrap();
    assert!(state.entities.row(bee_slot).bee.is_some());
    let source = DamageSource::Melee {
        attacker: bee,
        position: centre,
        yaw: 0.0,
        bonus_knockback: 0.0,
        sweep: false,
        fire_ticks: 0,
        bane_ticks: 0,
    };
    for _ in 0..2 {
        let slot = state.entities.slot_of(victim).unwrap();
        state.merge_damage.push((
            slot,
            DamagePiece {
                physical: None,
                custom_armor: false,
                amount: 2.0,
                kind: DamageKind::Melee,
                source: Some(source),
            },
        ));
        state.commit_merge_effects();
        assert_eq!(
            state
                .entities
                .row(state.entities.slot_of(victim).unwrap())
                .appearance
                .stinger_count,
            1
        );
        assert!(
            state
                .entities
                .row(state.entities.slot_of(bee).unwrap())
                .bee
                .as_ref()
                .unwrap()
                .stung
        );
        state.tick += 11;
    }
}

#[test]
fn embedded_projectiles_decay_has_vanilla_boundary_and_no_appearance_churn() {
    let (mut state, player, _) = setup();
    let slot = state.entities.slot_of(player).unwrap();
    let row = state.entities.row_mut(slot);
    let appearance = Arc::make_mut(&mut row.appearance);
    appearance.arrow_count = 1;
    appearance.stinger_count = 2;
    let unchanged = Arc::clone(&row.appearance);
    for _ in 0..559 {
        assert!(!advance(row));
        assert!(Arc::ptr_eq(&unchanged, &row.appearance));
    }
    assert!(advance(row)); // 20 * (30 - 2) = 560 for stingers.
    assert_eq!(
        (row.appearance.arrow_count, row.appearance.stinger_count),
        (1, 1)
    );
    for _ in 0..19 {
        assert!(!advance(row));
    }
    assert!(advance(row)); // 20 * (30 - 1) = 580 for arrows.
    assert_eq!(
        (row.appearance.arrow_count, row.appearance.stinger_count),
        (0, 1)
    );
}

#[test]
fn embedded_projectiles_existing_timer_is_not_reset_by_another_hit() {
    let mut timer = 10;
    assert!(!decay(2, &mut timer));
    assert_eq!(timer, 9);
    let mut timer = 0;
    assert!(decay(30, &mut timer));
    assert_eq!(timer, -1);
    assert!(decay(31, &mut timer));
    assert_eq!(timer, -21);
}

#[test]
fn embedded_projectiles_counts_are_not_in_the_durable_appearance_image() {
    let mut appearance = EntityAppearance::default();
    let before = crate::journal::EntityAppearanceImage::from(&appearance);
    appearance.arrow_count = 4;
    appearance.stinger_count = 2;
    assert_eq!(
        crate::journal::EntityAppearanceImage::from(&appearance),
        before
    );
}
