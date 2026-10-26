use crate::bees::tests::{POS, place, state};
use crate::boundary::ChunkPos;

/// A corrupt record installed in a resident chunk (the setter marks the chunk
/// for hydration) must unload that chunk with a load error, never crash the
/// tick thread (formerly `panic!`/`expect` or a silent skip in each owner).
#[test]
fn corrupt_block_entity_rejects_its_chunk_instead_of_panicking() {
    for block in [
        "jukebox",
        "crafter",
        "oak_shelf",
        "beacon",
        "lectern",
        "campfire",
        "brewing_stand",
        "hopper",
        "decorated_pot",
    ] {
        let mut state = state(None);
        place(&mut state, POS, &format!("minecraft:{block}"));
        let cell = (POS.x, POS.y, POS.z);
        state.jukeboxes.remove(&cell);
        state.crafters.remove(&cell);
        state.lecterns.remove(&cell);
        state.campfires.remove(&cell);
        state.brewing.remove(&cell);
        state.block_entities.remove(&cell);
        state
            .world
            .set_opaque_block_entity(POS, Some(vec![mc_nbt::tag_type::COMPOUND]));
        super::sync_chunk_block_entities(&mut state);
        let chunk = ChunkPos {
            x: POS.x.div_euclid(16),
            z: POS.z.div_euclid(16),
        };
        assert!(!state.world.has_chunk(chunk.x, chunk.z), "{block}");
        assert!(state.world.load_error(chunk).is_some(), "{block}");
    }
}
