use super::super::definition::{DefinitionInput, PlaneBounds};
use super::super::hydrology::Depression;
use super::super::world_plan::WorldPlan;
use super::*;

fn fixture() -> (PlanePlan, WorldIdentity) {
    let mut input = DefinitionInput::earth_root(712816);
    input.domain = PlaneBounds {
        min_x: 0,
        max_x_exclusive: 1200,
        min_z: 0,
        max_z_exclusive: 500,
    };
    input.macro_cell_m = 100.0;
    input.climate_cell_m = 200.0;
    let definition = input.validate().unwrap();
    let identity = WorldIdentity::derive(&definition, [3; 32], [4; 32]).unwrap();
    let WorldPlan::Plane(mut root) = WorldPlan::build(&definition).unwrap() else {
        panic!()
    };
    root.elevation_m.fill(100.0);
    root.ocean.fill(false);
    root.tectonics.erodibility.fill(0.8);
    root.climate_forcing.wind_m_s.fill([[0.0; 2]; 4]);
    root.hydrology.downstream.fill(None);
    root.hydrology.discharge_m3.fill([0.0; 4]);
    root.hydrology.water_level_m.fill(None);
    root.hydrology.seasonal_water_level_m.fill([None; 4]);
    root.hydrology.depression.fill(None);
    root.hydrology.depressions.clear();
    for pass in &mut root.erosion_passes {
        pass.transported_m3.fill([0.0; 2]);
    }
    (*root, identity)
}

fn connect(root: &mut PlanePlan, i: usize, j: usize, q: [f64; 4]) {
    root.hydrology.downstream[i] = Some(j);
    root.hydrology.discharge_m3[i] = q.map(|q| q * SECONDS_PER_SEASON);
}

fn patch(root: &PlanePlan, identity: WorldIdentity, i: usize) -> WaterGeometry {
    // These fixtures deliberately change immutable physical root fields;
    // rebuild their publication-time owner raster before a local query.
    let mut root = root.clone();
    root.water_body_owner = root_water_owners(&root.grid, &root.ocean, &root.hydrology).unwrap();
    prepare(&root, identity, cell_bounds(&root.grid, i)).unwrap()
}

fn surface(water: &WaterPlan) -> f64 {
    match water {
        WaterPlan::Ocean { surface_m, .. }
        | WaterPlan::Lake { surface_m, .. }
        | WaterPlan::River { surface_m, .. } => *surface_m,
        WaterPlan::None => panic!("expected owned water footprint"),
    }
}

#[test]
fn mountain_profiles_share_exact_confluence_and_carve_protected_beds() {
    let (mut root, identity) = fixture();
    let junction = 28;
    for (source, elevation, q) in [(27, 650.0, 8.0), (29, 920.0, 13.0), (16, 870.0, 5.0)] {
        root.elevation_m[source] = elevation;
        connect(&mut root, source, junction, [q; 4]);
    }
    root.elevation_m[40] = 20.0;
    connect(&mut root, junction, 40, [26.0; 4]);
    let geometry = patch(&root, identity, junction);
    let junction_anchor = anchor(&root, junction);
    for reach in &geometry.reaches {
        for edge in reach.points.windows(2) {
            assert!(edge[0].surface_m >= edge[1].surface_m);
        }
        if reach.receiving_cell == junction {
            assert_eq!(*reach.points.last().unwrap(), junction_anchor);
        } else {
            assert_eq!(reach.points[0], junction_anchor);
        }
        for point in &reach.points {
            let ground = terrain_height(&root, point.x_m, point.z_m) + 1.0;
            let water = sample(&geometry, point.x_m, point.z_m, ground);
            assert!(water.protected);
            assert!(water.ground_m < point.surface_m);
            assert!((surface(&water.water) - point.surface_m).abs() < 1e-8);
        }
    }
    let confluence = sample(&geometry, junction_anchor.x_m, junction_anchor.z_m, 102.0);
    assert_eq!(surface(&confluence.water), 100.0);
    assert!(confluence.ground_m < 100.0);
}

#[test]
fn neighbour_queries_produce_identical_crossings_beds_ids_and_directions() {
    let (mut root, identity) = fixture();
    root.elevation_m[27] = 102.0;
    root.elevation_m[28] = 100.0;
    root.elevation_m[29] = 98.0;
    connect(&mut root, 27, 28, [2.0, 5.0, 0.0, 3.0]);
    connect(&mut root, 28, 29, [3.0, 7.0, 0.0, 4.0]);
    let west = patch(&root, identity, 27);
    let east = patch(&root, identity, 28);
    for z in [200.0, 220.0, 249.0, 250.0, 251.0, 280.0, 300.0] {
        assert_eq!(
            sample(&west, 400.0, z, 105.0),
            sample(&east, 400.0, z, 105.0)
        );
    }
    let crossing = sample(&west, 400.0, 250.0, 105.0);
    let WaterPlan::River {
        surface_m,
        seasonal_q_m3_s,
        hydroperiod,
        ..
    } = crossing.water
    else {
        panic!()
    };
    assert_eq!(surface_m, 101.0);
    assert_eq!(seasonal_q_m3_s, [2.0, 5.0, 0.0, 3.0]);
    assert_eq!(hydroperiod.wet_seasons, [true, true, false, true]);
    let south = patch(&root, identity, 40);
    assert_eq!(
        sample(&east, 400.0, 300.0, 102.0),
        sample(&south, 400.0, 300.0, 102.0)
    );
    let tiny = prepare(&root, identity, [399.0, 400.0, 249.0, 250.0]).unwrap();
    assert_eq!(
        sample(&west, 400.0, 250.0, 105.0),
        sample(&tiny, 400.0, 250.0, 105.0)
    );
}

#[test]
fn meander_arc_is_bounded_by_cell_corridor_and_extra_incision_budget() {
    let (mut root, identity) = fixture();
    root.elevation_m[27] = 101.0;
    root.elevation_m[28] = 100.0;
    // High, unrelated divides on both sides must suppress expensive bends.
    for i in [15, 16, 39, 40] {
        root.elevation_m[i] = 10_000.0;
    }
    connect(&mut root, 27, 28, [3.0; 4]);
    let geometry = patch(&root, identity, 27);
    let reach = geometry
        .reaches
        .iter()
        .find(|r| r.source_cell == 27)
        .unwrap();
    for edge in reach.points.windows(2) {
        for k in 0..=32 {
            let point = interpolate(&edge[0], &edge[1], k as f64 / 32.0);
            let cell = root.grid.index_at(point.x_m, point.z_m);
            assert!(cell == 27 || cell == 28);
            assert!(
                terrain_height(&root, point.x_m, point.z_m) - point.surface_m
                    <= 2.0 + reach.depth_m * 4.0 + 1e-8
            );
        }
    }
}

fn coast(depth: f64, wind: f64, sediment: f64) -> (PlanePlan, WorldIdentity) {
    let (mut root, identity) = fixture();
    root.elevation_m[27] = 1.0;
    root.elevation_m[28] = -depth;
    root.ocean[28] = true;
    root.climate_forcing.wind_m_s[28] = [[wind, 0.0]; 4];
    root.erosion_passes.last_mut().unwrap().transported_m3[27] = [sediment * 0.6, sediment * 0.4];
    connect(&mut root, 27, 28, [10.0, 20.0, 0.0, 5.0]);
    (root, identity)
}

#[test]
fn shallow_sediment_rich_quiet_coast_infers_conservative_delta_network() {
    let (root, identity) = coast(3.0, 2.0, 300_000.0);
    let geometry = patch(&root, identity, 27);
    let stem = geometry
        .reaches
        .iter()
        .find(|r| r.split_id.is_none())
        .unwrap();
    assert_eq!(stem.mouth_regime, MouthRegime::Delta);
    let branches: Vec<_> = geometry
        .reaches
        .iter()
        .filter(|r| r.split_id.is_some())
        .collect();
    assert_eq!(branches.len(), 3);
    let split = *stem.points.last().unwrap();
    assert_eq!(split.surface_m, 0.0);
    assert_eq!([split.x_m, split.z_m], shared_edge(&root.grid, 27, 28));
    for branch in &branches {
        assert_eq!(branch.points[0], split);
        assert_eq!(branch.split_id, branches[0].split_id);
        for edge in branch.points.windows(2) {
            for k in 0..=16 {
                let point = interpolate(&edge[0], &edge[1], k as f64 / 16.0);
                assert_eq!(root.grid.index_at(point.x_m, point.z_m), 28);
                assert_eq!(point.surface_m, 0.0);
                assert!((point.x_m - split.x_m).hypot(point.z_m - split.z_m) < 60.0);
            }
        }
    }
    for season in 0..4 {
        let fraction: f64 = branches.iter().map(|b| b.seasonal_fraction[season]).sum();
        let q: f64 = branches.iter().map(|b| b.seasonal_q_m3_s[season]).sum();
        assert_eq!(fraction, 1.0);
        assert_eq!(q, stem.seasonal_q_m3_s[season]);
    }
    let receiving = patch(&root, identity, 28);
    for branch in branches {
        for point in &branch.points {
            assert_eq!(
                sample(&geometry, point.x_m, point.z_m, -3.0),
                sample(&receiving, point.x_m, point.z_m, -3.0)
            );
        }
    }
}

#[test]
fn deep_reworked_and_sediment_poor_mouths_are_static_estuaries_not_deltas() {
    for (depth, wind, sediment) in [
        (70.0, 2.0, 300_000.0),
        (3.0, 14.0, 300_000.0),
        (3.0, 2.0, 0.0),
    ] {
        let (root, identity) = coast(depth, wind, sediment);
        let geometry = patch(&root, identity, 27);
        assert_eq!(geometry.reaches.len(), 1);
        let reach = &geometry.reaches[0];
        assert_eq!(reach.mouth_regime, MouthRegime::StaticEstuary);
        assert!(reach.split_id.is_none());
        assert_eq!(reach.points.last().unwrap().surface_m, 0.0);
        assert_eq!(reach.points.last().unwrap().x_m, 400.0);
    }
    let (mut inland, identity) = fixture();
    inland.elevation_m[27] = 101.0;
    connect(&mut inland, 27, 28, [10.0; 4]);
    inland.erosion_passes.last_mut().unwrap().transported_m3[27] = [1e9; 2];
    assert_eq!(
        patch(&inland, identity, 27).reaches[0].mouth_regime,
        MouthRegime::None
    );
}

fn lake(root: &mut PlanePlan, cells: &[usize], spill_from: usize, spill_to: usize) {
    root.hydrology.depressions.push(Depression {
        id: 0,
        parent: None,
        children: Vec::new(),
        cells: cells.to_vec(),
        minimum_m: 0.0,
        spill_level_m: 10.0,
        spill_from,
        spill_to,
        capacity_m3: 1e6,
        storage_m3: [1e6; 4],
        level_m: [Some(10.0); 4],
        inflow_m3: [0.0; 4],
        outflow_m3: [0.0; 4],
        evaporation_m3: [0.0; 4],
        groundwater_export_m3: [0.0; 4],
    });
    for &i in cells {
        root.elevation_m[i] = 2.0;
        root.hydrology.depression[i] = Some(0);
        root.hydrology.seasonal_water_level_m[i] = [Some(10.0); 4];
        root.hydrology.water_level_m[i] = Some(10.0);
    }
}

#[test]
fn large_lake_uses_shared_horizontal_body_and_exact_inlet_outlet_anchors() {
    let (mut root, identity) = fixture();
    let cells: Vec<_> = (25..=34).collect();
    lake(&mut root, &cells, 34, 35);
    root.elevation_m[24] = 15.0;
    root.elevation_m[35] = 5.0;
    connect(&mut root, 24, 25, [2.0; 4]);
    connect(&mut root, 34, 35, [3.0; 4]);
    // Conceptual transport inside a lake must not excavate a second river.
    connect(&mut root, 27, 28, [100.0; 4]);
    let inlet = patch(&root, identity, 25);
    let outlet = patch(&root, identity, 34);
    let inlet_reach = inlet.reaches.iter().find(|r| r.source_cell == 24).unwrap();
    let outlet_reach = outlet.reaches.iter().find(|r| r.source_cell == 34).unwrap();
    assert_eq!(inlet_reach.points.last().unwrap().surface_m, 10.0);
    assert_eq!(outlet_reach.points[0].surface_m, 10.0);
    assert_eq!(inlet_reach.points.last().unwrap().x_m, 100.0);
    assert_eq!(outlet_reach.points[0].x_m, 1100.0);
    let WaterPlan::Lake {
        body_id: a,
        surface_m: ha,
        ..
    } = sample(&inlet, 150.0, 250.0, 2.0).water
    else {
        panic!()
    };
    let WaterPlan::Lake {
        body_id: b,
        surface_m: hb,
        ..
    } = sample(&outlet, 1050.0, 250.0, 2.0).water
    else {
        panic!()
    };
    assert_eq!(a, b);
    assert_eq!((ha, hb), (10.0, 10.0));
    assert!(inlet.footprints.len() < cells.len());
    assert!(inlet.reaches.iter().all(|r| r.source_cell != 27));
    root.hydrology.seasonal_water_level_m[25] = [None, Some(10.0), None, Some(10.0)];
    let shore = patch(&root, identity, 25);
    let WaterPlan::Lake {
        body_id: shore_id, ..
    } = sample(&shore, 150.0, 250.0, 2.0).water
    else {
        panic!()
    };
    assert_eq!(
        shore_id, a,
        "dry shore seasons must not create another body"
    );
}

#[test]
fn dry_losing_reaches_and_negative_land_do_not_create_water() {
    let (mut root, identity) = fixture();
    root.elevation_m[27] = -20.0;
    root.elevation_m[28] = -30.0;
    // Zero outgoing volume after evaporation/groundwater loss: no wet reach.
    connect(&mut root, 27, 28, [0.0; 4]);
    root.hydrology.evaporation_m3[27] = [100.0; 4];
    root.hydrology.groundwater_export_m3[27] = [200.0; 4];
    let geometry = patch(&root, identity, 27);
    for (x, z) in [(350.0, 250.0), (400.0, 250.0), (450.0, 250.0)] {
        let dry = sample(&geometry, x, z, -25.0);
        assert_eq!(dry.water, WaterPlan::None);
        assert_eq!(dry.ground_m, -25.0);
        assert!(!dry.protected);
    }
    root.ocean[28] = true;
    let geometry = patch(&root, identity, 28);
    assert_eq!(sample(&geometry, 450.0, 250.0, 0.0).water, WaterPlan::None);
    assert!(matches!(
        sample(&geometry, 450.0, 250.0, -0.5).water,
        WaterPlan::Ocean { .. }
    ));
    assert_eq!(
        sample(&geometry, 350.0, 250.0, -25.0).water,
        WaterPlan::None
    );
}

#[test]
fn hydraulic_width_responds_to_q_slope_and_material_without_block_width_cap() {
    let (mut root, identity) = fixture();
    root.elevation_m[27] = 100.001;
    connect(&mut root, 27, 28, [0.02; 4]);
    let stream = patch(&root, identity, 27).reaches[0].width_m;
    connect(&mut root, 27, 28, [100.0; 4]);
    let broad = patch(&root, identity, 27).reaches[0].width_m;
    root.elevation_m[27] = 700.0;
    let steep = patch(&root, identity, 27).reaches[0].width_m;
    root.tectonics.erodibility[27] = 0.1;
    let rock = patch(&root, identity, 27).reaches[0].width_m;
    assert!(stream < 1.0);
    assert!(broad > stream * 10.0);
    assert!(steep < broad);
    assert!(rock < steep);
    // Same physical law in a 16 km macro patch can exceed hundreds of metres.
    root.grid.step_m *= 160.0;
    root.grid.width_m *= 160.0;
    root.grid.depth_m *= 160.0;
    root.elevation_m[27] = 100.001;
    root.tectonics.erodibility[27] = 0.8;
    connect(&mut root, 27, 28, [100_000.0; 4]);
    assert!(patch(&root, identity, 27).reaches[0].width_m > 1000.0);
}

#[test]
fn candidate_work_is_bounded_independently_of_root_catalog_and_upstream_length() {
    let (mut root, identity) = fixture();
    for i in 24..35 {
        root.elevation_m[i] = (36 - i) as f64;
        connect(&mut root, i, i + 1, [1.0; 4]);
    }
    root.elevation_m[35] = 1.0;
    let geometry = patch(&root, identity, 34);
    assert!(geometry.candidate_cells <= MAX_CANDIDATE_CELLS);
    assert!(geometry.reaches.len() <= MAX_REACH_FEATURES);
    assert!(geometry.feature_count() <= MAX_REACH_FEATURES + MAX_CANDIDATE_CELLS);
    assert!(geometry.reaches.iter().all(|r| r.source_cell >= 32));
    // Unrelated root-owned catalogs are not candidates or traversed trees.
    let before = geometry.clone();
    let large = vec![0; 100_000];
    root.hydrology.depressions.push(Depression {
        id: 0,
        parent: None,
        children: large.clone(),
        cells: large,
        minimum_m: 0.0,
        spill_level_m: 1.0,
        spill_from: 0,
        spill_to: 1,
        capacity_m3: 1.0,
        storage_m3: [0.0; 4],
        level_m: [None; 4],
        inflow_m3: [0.0; 4],
        outflow_m3: [0.0; 4],
        evaporation_m3: [0.0; 4],
        groundwater_export_m3: [0.0; 4],
    });
    assert_eq!(patch(&root, identity, 34), before);
    assert!(prepare(&root, identity, [0.0, 200.0, 0.0, 100.0]).is_err());
}

#[test]
fn partial_outer_cells_and_domain_edges_have_shared_clip_contracts() {
    let (mut root, identity) = fixture();
    root.grid.width_m = 1150.0;
    root.grid.depth_m = 450.0;
    root.elevation_m[58] = 101.0;
    connect(&mut root, 58, 59, [2.0; 4]);
    let left = patch(&root, identity, 58);
    let right = patch(&root, identity, 59);
    for (x, z) in [
        (1100.0, 425.0),
        (1100.0, 450.0),
        (1150.0, 425.0),
        (1150.0, 450.0),
    ] {
        assert_eq!(sample(&left, x, z, 102.0), sample(&right, x, z, 102.0));
    }
    assert_eq!(right.grid.center(59), (1125.0, 425.0));
    assert!(right.candidate_cells <= MAX_CANDIDATE_CELLS);
    let encoded = serde_json::to_vec(&right).unwrap();
    let decoded: WaterGeometry = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, right);
    assert_eq!(
        sample(&decoded, 1100.0, 425.0, 102.0),
        sample(&right, 1100.0, 425.0, 102.0)
    );
}

#[test]
fn earlier_wet_final_dry_water_does_not_materialize_or_create_an_outlet() {
    let (mut root, identity) = fixture();
    root.elevation_m[27] = 15.0;
    root.elevation_m[28] = -10.0;
    connect(&mut root, 27, 28, [2.0, 5.0, 1.0, 0.0]);
    let dry_reach = patch(&root, identity, 27);
    assert!(dry_reach.reaches.is_empty());
    let dry = sample(&dry_reach, 400.0, 250.0, -12.0);
    assert_eq!(dry.water, WaterPlan::None);
    assert_eq!(dry.ground_m, -12.0);
    lake(&mut root, &[28], 28, 29);
    root.hydrology.seasonal_water_level_m[28] = [Some(10.0), Some(8.0), Some(5.0), None];
    root.hydrology.water_level_m[28] = None;
    let dry_lake = patch(&root, identity, 28);
    assert_eq!(sample(&dry_lake, 450.0, 250.0, 2.0).water, WaterPlan::None);
    assert_eq!(anchor(&root, 28).surface_m, root.elevation_m[28]);
    let context = dry_lake
        .footprints
        .iter()
        .find(|p| p.root_cell == 28)
        .unwrap();
    assert_eq!(
        context.seasonal_surface_m,
        [Some(10.0), Some(8.0), Some(5.0), None]
    );
    // A still-wet incoming river is not expanded as a delta/estuary into a
    // body that was present only in earlier seasons.
    connect(&mut root, 27, 28, [2.0; 4]);
    let incoming = patch(&root, identity, 27);
    assert_eq!(incoming.reaches[0].mouth_regime, MouthRegime::None);
    assert_eq!(incoming.reaches[0].points.last().unwrap().surface_m, 2.0);
}

#[test]
fn connected_body_owners_distinguish_oceans_and_merge_nested_lake_footprints() {
    let (mut root, identity) = fixture();
    // Irregular connected sea reaches around a dry corner; a nearby isolated
    // sea is a different body, regardless of straight-line seed distance.
    for i in [13, 14, 26, 38, 39, 41] {
        root.ocean[i] = true;
        root.elevation_m[i] = -5.0;
    }
    lake(&mut root, &[28, 29, 30], 30, 31);
    // Distinct hierarchy references do not split a connected horizontal lake.
    root.hydrology.depression[29] = Some(1);
    root.hydrology.depression[30] = Some(2);
    root.hydrology.seasonal_water_level_m[30][0] = Some(11.0);
    root.water_body_owner = root_water_owners(&root.grid, &root.ocean, &root.hydrology).unwrap();
    assert_eq!(root.water_body_owner[39], [Some(13); 4]);
    assert_eq!(root.water_body_owner[41], [Some(41); 4]);
    assert_eq!(root.water_body_owner[28], [Some(28); 4]);
    assert_eq!(root.water_body_owner[29], [Some(28); 4]);
    assert_eq!(
        root.water_body_owner[30],
        [Some(30), Some(28), Some(28), Some(28)]
    );
    let geometry = prepare(&root, identity, cell_bounds(&root.grid, 29)).unwrap();
    let sea_a = sample(&geometry, 350.0, 350.0, -5.0).water;
    let sea_b = sample(&geometry, 550.0, 350.0, -5.0).water;
    let (WaterPlan::Ocean { body_id: a, .. }, WaterPlan::Ocean { body_id: b, .. }) = (sea_a, sea_b)
    else {
        panic!()
    };
    assert_ne!(a, b);
    let mut lake_id = None;
    for cell in [28, 29, 30] {
        let (x, z) = root.grid.center(cell);
        let WaterPlan::Lake {
            body_id, surface_m, ..
        } = sample(&geometry, x, z, 2.0).water
        else {
            panic!()
        };
        assert_eq!(surface_m, 10.0);
        if let Some(id) = lake_id {
            assert_eq!(id, body_id);
        }
        lake_id = Some(body_id);
    }
}

#[test]
fn delta_channel_carving_is_clipped_to_supported_sediment_fan() {
    let (root, identity) = coast(3.0, 2.0, 20_000.0);
    let geometry = patch(&root, identity, 27);
    let branch = geometry
        .reaches
        .iter()
        .find(|r| r.split_id.is_some())
        .unwrap();
    let footprint = branch.sediment_footprint.unwrap();
    for reach in geometry.reaches.iter().filter(|r| r.split_id.is_some()) {
        for point in &reach.points {
            assert!(
                (point.x_m - footprint.split_m[0]).hypot(point.z_m - footprint.split_m[1])
                    <= footprint.radius_m + 1e-8
            );
        }
    }
    // The receiving ocean cell includes this emergent shoal, but neither
    // body membership nor a nearby branch licenses carving past the fan.
    let outside_x = footprint.split_m[0] + footprint.radius_m + 1.0;
    let unsupported = sample(&geometry, outside_x, footprint.split_m[1], 0.25);
    assert_eq!(unsupported.ground_m, 0.25);
    assert_eq!(unsupported.water, WaterPlan::None);
    let central = geometry
        .reaches
        .iter()
        .find(|r| r.split_id.is_some() && r.seasonal_fraction == [0.5; 4])
        .unwrap();
    let point = central.points[8];
    let supported = sample(&geometry, point.x_m, point.z_m, 0.25);
    assert!(supported.ground_m < 0.0);
    assert!(matches!(supported.water, WaterPlan::Ocean { .. }));
}
