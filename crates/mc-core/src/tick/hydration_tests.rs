use std::collections::BTreeMap;

use crate::bees::tests::{POS, place, state};
use crate::boundary::ChunkPos;

/// A corrupt persisted record must unload its chunk with a load error when
/// the chunk becomes resident, never crash the tick thread (formerly
/// `panic!`/`expect` in each owner).
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
    ] {
        let dir = std::env::temp_dir().join(format!("hydrate-{block}-{}", uuid::Uuid::new_v4()));
        let mut original = state(Some(dir.clone()));
        place(&mut original, POS, &format!("minecraft:{block}"));
        original.world.fold_overlay();
        original
            .world
            .set_opaque_block_entity(POS, Some(vec![mc_nbt::tag_type::COMPOUND]));
        original
            .world
            .capture_dirty(&BTreeMap::new())
            .write()
            .unwrap();
        drop(original);
        let mut restored = state(Some(dir.clone()));
        super::sync_chunk_block_entities(&mut restored);
        let chunk = ChunkPos {
            x: POS.x.div_euclid(16),
            z: POS.z.div_euclid(16),
        };
        assert!(!restored.world.has_chunk(chunk.x, chunk.z), "{block}");
        assert!(restored.world.load_error(chunk).is_some(), "{block}");
        assert!(
            restored.jukeboxes.is_empty() && restored.crafters.is_empty(),
            "{block}"
        );
        drop(restored);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
