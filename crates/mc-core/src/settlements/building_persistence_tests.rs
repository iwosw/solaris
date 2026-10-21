//! Building persistence regression: nonempty certified Storage roundtrips
//! through the touched-only settlement WAL image and per-record checkpoint
//! row. Row/wal validation is shape-only (id linkage, bounds size, sequence
//! fence, sorted in-bounds points); world liveness is never inspected here.
use super::*;
use crate::journal::{JournalEntry, Replay};

fn state() -> TickState {
    let registry = std::sync::Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let generator = std::sync::Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        std::sync::Arc::clone(&registry.blocks),
    ));
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 64, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 2, 512)
}

fn building(
    id: super::super::buildings::BuildingId,
    settlement: SettlementId,
) -> super::super::buildings::Building {
    super::super::buildings::Building {
        id,
        settlement,
        kind: super::super::buildings::BuildingKind::Storage,
        level: 1,
        bounds: mc_script::ScriptWorldBounds::new([3, 200, 3], [7, 204, 7]).unwrap(),
        work_points: vec![[4, 201, 4]],
        revision: 1,
        natural_zone: false,
    }
}

#[test]
fn nonempty_building_roundtrips_touched_wal_and_checkpoint_row() {
    let mut state = state();
    let settlement = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "buildings".into(),
            "owner".into(),
            [8, 201, 8],
            48,
        )
        .unwrap();
    let id = super::super::buildings::BuildingId((settlement.0 << 32) | 1);
    {
        let record = state.settlements.get_mut(settlement).unwrap();
        record.building_sequence = 1;
        record.buildings.insert(id, building(id, settlement));
        record.revision += 1;
    }
    let before = state.settlements.get(settlement).unwrap().clone();

    let mut entries = Vec::new();
    journal_entries(&mut state, 2, &mut entries);
    assert_eq!(entries.len(), 1);
    assert!(matches!(
        &entries[0],
        JournalEntry::Settlement { image, .. }
            if image.id == settlement
                && image.value.as_ref().is_some_and(|v| *v == before)
    ));
    let mut idle = Vec::new();
    journal_entries(&mut state, 3, &mut idle);
    assert!(idle.is_empty(), "drain must capture touched once");

    let rows = checkpoint_rows(&state, 4).unwrap();
    let row: Row = serde_json::from_slice(&rows[&row_key(settlement)].payload).unwrap();
    assert_eq!(row.value, before);
    assert_eq!(row.value.building_sequence, 1);
    assert_eq!(row.value.buildings[&id].work_points, vec![[4, 201, 4]]);

    let replay = crate::journal::apply_batch_images(Replay::default(), &[(2, 2, entries)]);
    let mut restored = self::state();
    // Recover from WAL alone, not the already equivalent checkpoint row.
    hydrate_dimension(
        &mut restored,
        &std::collections::BTreeMap::new(),
        &replay,
        settlement.0,
    )
    .unwrap();
    finish_recovery(&mut restored).unwrap();
    assert_eq!(restored.settlements.get(settlement).unwrap(), &before);

    let rows = checkpoint_rows(&restored, 5).unwrap();
    let mut reopened = self::state();
    hydrate_dimension(&mut reopened, &rows, &Replay::default(), 5).unwrap();
    finish_recovery(&mut reopened).unwrap();
    assert_eq!(reopened.settlements.get(settlement).unwrap(), &before);
}

#[test]
fn legacy_row_without_buildings_decodes_to_empty_map() {
    let mut state = state();
    let settlement = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "legacy".into(),
            "owner".into(),
            [0, 64, 0],
            48,
        )
        .unwrap();
    let record = state.settlements.get(settlement).unwrap().clone();
    let mut value = serde_json::to_value(&record).unwrap();
    for key in ["building_sequence", "buildings"] {
        value.as_object_mut().unwrap().remove(key);
    }
    let row = Row {
        dimension: crate::dimensions::Dimension::Overworld,
        value,
    };
    let stored = crate::native_checkpoint::record(
        6,
        serde_json::to_vec(&row).expect("legacy-shaped row serializes"),
    );
    let mut rows = std::collections::BTreeMap::new();
    rows.insert(row_key(settlement), stored);
    let mut reopened = self::state();
    hydrate_dimension(&mut reopened, &rows, &Replay::default(), settlement.0).unwrap();
    finish_recovery(&mut reopened).unwrap();
    let restored = reopened.settlements.get(settlement).unwrap();
    assert_eq!(restored.building_sequence, 0);
    assert!(restored.buildings.is_empty());
}
