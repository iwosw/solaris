use super::*;
use crate::vanilla_features::{BlockTagIndex, LegacyRandom};
use mc_data::vanilla_feature_closure::IntProviderSpec;
use mc_world::BlockRegistry;
use std::collections::HashMap;

struct Tags;
impl BlockTagIndex for Tags {
    fn block_in_tag(&self, tag: &Identifier, block: &Identifier) -> bool {
        tag.as_str() == "minecraft:sculk_replaceable_world_gen"
            && block.as_str() == "minecraft:stone"
    }
}
struct Level {
    cells: HashMap<BlockPos, BlockStateId>,
    air: BlockStateId,
    stone: BlockStateId,
}
impl FeatureLevel for Level {
    fn min_y(&self) -> i32 {
        -64
    }
    fn max_y(&self) -> i32 {
        320
    }
    fn block_state(&self, pos: BlockPos) -> BlockStateId {
        self.cells
            .get(&pos)
            .copied()
            .unwrap_or(if pos.y < 64 { self.stone } else { self.air })
    }
    fn set_block(&mut self, pos: BlockPos, state: BlockStateId) {
        self.cells.insert(pos, state);
    }
    fn is_water_source_at(&self, _: BlockPos) -> bool {
        false
    }
}
#[test]
fn modern_jigsaw_sculk_patch_is_seeded_bounded_and_preserves_protected_blocks() {
    let blocks =
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap();
    let semantics = BlockSemantics::new(&blocks, &Tags);
    let owner = Identifier::parse("minecraft:sculk_fixture").unwrap();
    let spec = SculkPatchSpec {
        charge_count: 10,
        amount_per_charge: 32,
        spread_attempts: 64,
        growth_rounds: 0,
        spread_rounds: 1,
        extra_rare_growths: IntProviderSpec::Constant(2),
        catalyst_chance: 1.0,
    };
    let feature = CompiledSculkPatch::compile(&semantics, &owner, &spec).unwrap();
    let state = |name| {
        blocks
            .block(&Identifier::parse(name).unwrap())
            .unwrap()
            .default
    };
    let protected = BlockPos { x: 1, y: 63, z: 0 };
    let run = || {
        let mut level = Level {
            cells: HashMap::from([(protected, state("minecraft:obsidian"))]),
            air: state("minecraft:air"),
            stone: state("minecraft:stone"),
        };
        assert!(feature.place(
            &mut level,
            &semantics,
            &mut LegacyRandom::new(4242),
            BlockPos { x: 0, y: 64, z: 0 }
        ));
        level
    };
    let level = run();
    assert_eq!(level.cells, run().cells);
    assert_eq!(level.block_state(protected), state("minecraft:obsidian"));
    let sculk = level
        .cells
        .values()
        .filter(|value| **value == feature.sculk)
        .count();
    assert!((1..=320).contains(&sculk));
    assert!(level.cells.values().any(|value| *value == feature.catalyst));
    assert_eq!(
        level
            .cells
            .values()
            .filter(|value| **value == feature.shrieker[0])
            .count(),
        2
    );
    assert!(
        blocks
            .by_id(feature.shrieker[0])
            .unwrap()
            .properties
            .iter()
            .any(|(key, value)| key == "can_summon" && value == "true")
    );
}
