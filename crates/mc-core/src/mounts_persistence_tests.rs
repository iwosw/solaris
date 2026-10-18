use super::*;
use crate::journal::{
    EntityStateImage, JournalWriter, Replay, read_complete_batches_after, replay_batches,
};
use crate::mounts::tests::{fixture, hold, slot, spawn, use_entity};
use crate::tick::run_tick;
#[test]
fn actual_region_checkpoint_and_wal_tail_restore_tame_owner_collar_and_equipment() {
    let (mut state, actor, pos) = fixture();
    let wolf = spawn(&mut state, "wolf", pos);
    let w = slot(&state, wolf);
    hold(&mut state, actor, "bone", 64);
    for _ in 0..32 {
        run_tick(&mut state, &[use_entity(wolf)]);
        if state.entities.row(w).entity_life.tamed {
            break;
        }
    }
    assert!(state.entities.row(w).entity_life.tamed);
    hold(&mut state, actor, "wolf_armor", 1);
    run_tick(&mut state, &[use_entity(wolf)]);
    let (_, regions, _) = checkpoint_data_for(&state, state.tick);
    let dir = std::env::temp_dir().join(format!("solaris-mount-restart-{}", uuid::Uuid::new_v4()));
    mc_world::anvil::entity_regions::write_entity_regions(
        &dir,
        &state.registry.items,
        &regions,
        state.tick as u32,
    )
    .unwrap();
    let read =
        mc_world::anvil::entity_regions::read_entity_regions(&dir, &state.registry.items).unwrap();
    let native = read
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|e| e.id == "minecraft:wolf")
        .unwrap();
    let image = EntityStateImage::from_record(&state.registry, native).unwrap();
    assert_eq!(
        image.entity_life.taming,
        state.entities.row(w).entity_life.taming
    );
    let mut checkpoint = Replay::default();
    checkpoint.entities.insert(
        wolf.0,
        crate::journal::ReplayEntity {
            uuid: native.uuid.to_string(),
            type_id: crate::mounts::type_id(state.entities.body(w)).unwrap(),
            pos: native.pos,
            vel: velocity_from_vanilla_motion(native.motion),
            yaw: native.yaw,
            pitch: native.pitch,
            ground: native.on_ground,
            health: native.health,
            alive: true,
            age: image.age,
            breed_cooldown: image.breed_cooldown,
            pickup_delay: 0,
            item: None,
            appearance: appearance_from_vanilla_entity(native),
            active_effects: Default::default(),
            villager: None,
            projectile: None,
            state: image,
            tick: state.tick,
        },
    );
    hold(&mut state, actor, "blue_dye", 1);
    let (out, _, _) = run_tick(&mut state, &[use_entity(wolf)]);
    let entries = journal_entries_for_tick(&mut state, &out);
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(1, state.tick, &entries).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    let replay = replay_batches(checkpoint, &batches);
    let (mut restored, _, _) = fixture();
    restore_entity(&mut restored, wolf.0, &replay.entities[&wolf.0]).unwrap();
    let recovered = restored.entities.row(slot(&restored, wolf));
    assert_eq!(
        recovered.entity_life.taming,
        state.entities.row(w).entity_life.taming
    );
    assert_eq!(
        recovered.entity_life.sitting,
        state.entities.row(w).entity_life.sitting
    );
    assert_eq!(
        recovered.loot.attributes,
        state.entities.row(w).loot.attributes
    );
    assert_eq!(
        recovered.loot.body_armor,
        state.entities.row(w).loot.body_armor
    );
    std::fs::remove_dir_all(dir).unwrap();
}
