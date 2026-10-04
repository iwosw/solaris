use super::super::definition::{DefinitionInput, PlaneBounds};
use super::super::hydrology::{self, ChannelLosses};
use super::super::world_plan::WorldPlan;
use super::*;

fn fixture() -> (PlanePlan, WorldIdentity) {
    let mut input = DefinitionInput::earth_root(42);
    input.domain = PlaneBounds {
        min_x: -256,
        max_x_exclusive: 256,
        min_z: -256,
        max_z_exclusive: 256,
    };
    input.macro_cell_m = 64.0;
    input.climate_cell_m = 128.0;
    let d = input.validate().unwrap();
    let plan = WorldPlan::build(&d).unwrap();
    let identity = WorldIdentity::derive(&d, [1; 32], [2; 32]).unwrap();
    (plan.plane().unwrap().clone(), identity)
}
fn node(root: u64) -> GeoNodeId {
    GeoNodeId {
        root,
        level: 0,
        x: 0,
        z: 0,
    }
}
fn budget_values(b: RegionBudget) -> Vec<f64> {
    let mut v = vec![b.area_m2];
    for a in [
        b.local_runoff_m3,
        b.reach_evaporation_m3,
        b.groundwater_export_m3,
        b.lake_evaporation_m3,
        b.lake_groundwater_export_m3,
        b.lake_storage_m3,
    ] {
        v.extend(a);
    }
    for a in [
        b.sediment_source_m3,
        b.sediment_deposited_m3,
        b.sediment_stored_m3,
    ] {
        v.extend(a);
    }
    v
}
fn conserved(planner: &RegionPlanner<'_>, parent: GeoNodeId) {
    let budget = budget_values(planner.prepare(parent).unwrap().budget);
    let children = (0..4)
        .map(|q| budget_values(planner.prepare(parent.child(q).unwrap()).unwrap().budget))
        .collect::<Vec<_>>();
    for (i, expected) in budget.iter().enumerate() {
        let actual = children.iter().map(|c| c[i]).sum::<f64>();
        assert_eq!(actual, *expected, "extensive budget component {i}");
    }
}
#[test]
fn p05_cold_path_has_fixed_ancestors_and_no_upstream_dependency() {
    let (root, identity) = fixture();
    let planner = RegionPlanner::new(&root, identity);
    let leaf = planner.leaf_at(-5.25, -6.75).unwrap();
    let (plan, profile) = planner.prepare_cold(leaf).unwrap();
    assert_eq!(profile.prepared_nodes, u32::from(leaf.level) + 1);
    assert_eq!(profile.prepared_nodes, 7);
    assert_eq!(
        profile.sampled_points,
        profile.prepared_nodes as usize * profile.bound.column_evaluations_per_node
    );
    assert!(
        profile.candidate_cells
            <= profile.prepared_nodes as usize * profile.bound.max_candidate_cells_per_node
    );
    assert_eq!(profile.upstream_traversals, 0);
    assert!(plan.bounds_m[1] - plan.bounds_m[0] <= PHYSICAL_DETAIL_M);
    assert_eq!(plan, planner.prepare(leaf).unwrap());
    // Mutate a root cell outside all fixed spatial halos, pretending it had a
    // huge never-visited upstream. It cannot affect this ready leaf's geometry.
    let mut remote = root.clone();
    remote.hydrology.discharge_m3[63] = [1e30; 4];
    remote.runoff.local_m3[63] = [1e30; 4];
    let other = RegionPlanner::new(&remote, identity).prepare(leaf).unwrap();
    assert_eq!(plan, other);
    let ready = plan.sample(-5.25, -6.75).unwrap();
    assert_eq!(ready, other.sample(-5.25, -6.75).unwrap());
}
#[test]
fn p06_sibling_edges_and_four_way_corners_are_order_independent() {
    let (mut root, identity) = fixture();
    let n = root.grid.len();
    root.elevation_m = (0..n)
        .map(|i| 800.0 - (i % 8) as f64 * 40.0 - (i / 8) as f64 * 10.0)
        .collect();
    root.ocean = vec![false; n];
    root.runoff.local_m3 = vec![[10000.0; 4]; n];
    root.hydrology = hydrology::solve(
        &root.grid,
        &root.elevation_m,
        &root.ocean,
        &root.runoff.local_m3,
    )
    .unwrap();
    root.water_body_owner =
        region_water::root_water_owners(&root.grid, &root.ocean, &root.hydrology).unwrap();
    let planner = RegionPlanner::new(&root, identity);
    let parent = node(27);
    let ids = (0..4).map(|q| parent.child(q).unwrap()).collect::<Vec<_>>();
    let forward = ids
        .iter()
        .map(|id| planner.prepare(*id).unwrap())
        .collect::<Vec<_>>();
    let reverse = ids
        .iter()
        .rev()
        .map(|id| planner.prepare(*id).unwrap())
        .collect::<Vec<_>>();
    assert!(
        forward
            .iter()
            .flat_map(|p| p.columns.iter())
            .any(|c| matches!(c.water, WaterPlan::River { .. }))
    );
    for i in 0..4 {
        assert_eq!(forward[i], reverse[3 - i]);
    }
    assert_eq!(forward[0].edges[1], forward[1].edges[0]);
    assert_eq!(forward[0].edges[3], forward[2].edges[2]);
    assert_eq!(forward[2].edges[1], forward[3].edges[0]);
    for (i, corner) in [(0, 3), (1, 2), (2, 1), (3, 0)] {
        assert_eq!(forward[0].corners[3], forward[i].corners[corner]);
    }
    let left = planner.prepare(node(27)).unwrap();
    let right = planner.prepare(node(28)).unwrap();
    assert_eq!(left.edges[1], right.edges[0]);
    let seam_x = left.bounds_m[1];
    let seam_z = (left.bounds_m[2] + left.bounds_m[3]) * 0.5;
    assert_eq!(
        left.sample(seam_x, seam_z).unwrap(),
        right.sample(seam_x, seam_z).unwrap()
    );
}
#[test]
fn p07_losing_reach_refinement_preserves_real_root_losses_and_solid_budgets() {
    let (mut root, identity) = fixture();
    let n = root.grid.len();
    root.elevation_m = (0..n)
        .map(|i| 500.0 - (i % 8) as f64 * 20.0 - (i / 8) as f64 * 10.0)
        .collect();
    root.ocean = vec![false; n];
    root.runoff.local_m3 = vec![[1000.0, 200.0, 0.0, 500.0]; n];
    let mut losses = ChannelLosses {
        evaporation_m3: vec![[0.0; 4]; n],
        groundwater_export_m3: vec![[0.0; 4]; n],
    };
    losses.evaporation_m3[27] = [100.0, 50.0, 0.0, 100.0];
    losses.groundwater_export_m3[27] = [200.0, 100.0, 0.0, 150.0];
    root.hydrology = hydrology::solve_with_losses(
        &root.grid,
        &root.elevation_m,
        &root.ocean,
        &root.runoff.local_m3,
        &losses,
    )
    .unwrap();
    root.water_body_owner =
        region_water::root_water_owners(&root.grid, &root.ocean, &root.hydrology).unwrap();
    root.hydrology
        .validate(
            &root.grid,
            &root.elevation_m,
            &root.ocean,
            &root.runoff.local_m3,
        )
        .unwrap();
    let pass = root.erosion_passes.last_mut().unwrap();
    pass.source_m3[27] = [31.0, 17.0];
    pass.deposited_m3[27] = [9.0, 4.0];
    pass.stored_m3[27] = [2.0, 3.0];
    let planner = RegionPlanner::new(&root, identity);
    let parent = planner.prepare(node(27)).unwrap();
    assert_eq!(
        parent.budget.reach_evaporation_m3,
        [100.0, 50.0, 0.0, 100.0]
    );
    assert_eq!(
        parent.budget.groundwater_export_m3,
        [200.0, 100.0, 0.0, 150.0]
    );
    assert_eq!(parent.budget.sediment_source_m3, [31.0, 17.0]);
    conserved(&planner, node(27));
    conserved(&planner, node(27).child(3).unwrap());
    let root_source = root.runoff.local_m3.iter().map(|v| v[0]).sum::<f64>();
    let loss = root
        .hydrology
        .evaporation_m3
        .iter()
        .map(|v| v[0])
        .sum::<f64>()
        + root
            .hydrology
            .groundwater_export_m3
            .iter()
            .map(|v| v[0])
            .sum::<f64>();
    assert!((root.hydrology.external_outflow_m3[0] + loss - root_source).abs() < 1e-7);
}
#[test]
fn p07_closed_lake_stores_input_without_child_outlets_or_water_creation() {
    let (mut root, identity) = fixture();
    let n = root.grid.len();
    root.elevation_m = vec![1000.0; n];
    root.elevation_m[27] = 200.0;
    root.ocean = vec![false; n];
    root.runoff.local_m3 = vec![[0.0; 4]; n];
    root.runoff.local_m3[27] = [100.0; 4];
    root.hydrology = hydrology::solve(
        &root.grid,
        &root.elevation_m,
        &root.ocean,
        &root.runoff.local_m3,
    )
    .unwrap();
    root.water_body_owner =
        region_water::root_water_owners(&root.grid, &root.ocean, &root.hydrology).unwrap();
    let depression = root.hydrology.depression[27].unwrap();
    assert_eq!(root.hydrology.depressions[depression].outflow_m3, [0.0; 4]);
    assert_eq!(
        root.hydrology.depressions[depression].storage_m3,
        [100.0, 200.0, 300.0, 400.0]
    );
    let planner = RegionPlanner::new(&root, identity);
    conserved(&planner, node(27));
    let parent = planner.prepare(node(27)).unwrap();
    for (actual, expected) in parent
        .budget
        .lake_storage_m3
        .into_iter()
        .zip([100.0, 200.0, 300.0, 400.0])
    {
        assert!((actual - expected).abs() < 1e-7);
    }
    let column = parent.sample(-32.0, -32.0).unwrap();
    let WaterPlan::Lake {
        surface_m, bed_m, ..
    } = column.water
    else {
        panic!("positive root lake stock must produce a wet protected centre");
    };
    assert_eq!(compact_y(surface_m), compact_y(bed_m));
    assert_eq!(column.bed_quantization_residual_y, -1);
    assert_eq!(column.bed_y, column.water_surface_y.map(|y| y - 1));
    assert_eq!(parent.budget.local_runoff_m3, [100.0; 4]);
    for q in 0..4 {
        let child = planner.prepare(node(27).child(q).unwrap()).unwrap();
        assert_eq!(child.budget.local_runoff_m3, [25.0; 4]);
        for (actual, expected) in child
            .budget
            .lake_storage_m3
            .into_iter()
            .zip([25.0, 50.0, 75.0, 100.0])
        {
            assert!((actual - expected).abs() < 1e-7);
        }
        for reach in &child.water_geometry.reaches {
            assert_ne!(
                reach.source_cell, 27,
                "closed lake cannot invent an outlet reach"
            );
        }
    }
    assert_eq!(parent.budget.local_runoff_m3, [100.0; 4]);
}
#[test]
fn physical_edges_negative_coordinates_and_partial_cells_have_checked_addresses() {
    let (mut root, identity) = fixture();
    root.grid.width_m = 501.0;
    root.grid.depth_m = 489.0;
    root.grid.validate().unwrap();
    let planner = RegionPlanner::new(&root, identity);
    let edge = planner.leaf_at(244.5, 232.5).unwrap();
    assert_eq!(edge.root, 63);
    let b = planner.bounds(edge).unwrap();
    assert!(b[0] >= -256.0 && b[1] <= 245.0 && b[3] <= 233.0);
    assert!(planner.leaf_at(245.0, 0.0).is_err());
    assert!(planner.leaf_at(f64::NAN, 0.0).is_err());
    assert!(
        planner
            .prepare(GeoNodeId {
                root: 0,
                level: 31,
                x: 0,
                z: 0
            })
            .is_err()
    );
    assert!(
        planner
            .prepare(GeoNodeId {
                root: 0,
                level: 1,
                x: 2,
                z: 0
            })
            .is_err()
    );
}

#[test]
fn published_physical_root_cohort_prepares_coastal_and_inland_regions_without_reversing_water() {
    for seed in [0_i64, 1, 2, 42, i64::MIN, i64::MAX] {
        let mut input = DefinitionInput::earth_root(seed);
        input.domain = PlaneBounds {
            min_x: -1_000_000,
            max_x_exclusive: 1_000_000,
            min_z: -1_000_000,
            max_z_exclusive: 1_000_000,
        };
        input.macro_cell_m = 250_000.0;
        input.climate_cell_m = 1_000_000.0;
        let definition = input.validate().unwrap();
        let world = WorldPlan::build(&definition).unwrap();
        let root = world.plane().unwrap();
        let identity = WorldIdentity::derive(&definition, [1; 32], [2; 32]).unwrap();
        let planner = RegionPlanner::new(root, identity);
        for patch in [0, 7, 27, 28, 56, 63] {
            let region = planner.prepare(node(patch)).unwrap_or_else(|error| {
                panic!("seed {seed}, published root patch {patch}: {error}")
            });
            for reach in &region.water_geometry.reaches {
                for points in reach.points.windows(2) {
                    assert!(points[0].surface_m >= points[1].surface_m);
                }
            }
            for column in &region.columns {
                if let Some(surface) = column.water_surface_y {
                    assert!(column.ground_y < surface);
                    assert!(column.bed_y.unwrap() < surface);
                }
            }
        }
    }
}

fn steep_ready_region() -> RegionPlan {
    let (mut root, identity) = fixture();
    let n = root.grid.len();
    root.elevation_m = (0..n)
        .map(|i| 10000.0 - (i % 8) as f64 * 400.0 - (i / 8) as f64 * 500.0)
        .collect();
    root.ocean = vec![false; n];
    root.runoff.local_m3 = vec![[10000.0; 4]; n];
    root.hydrology = hydrology::solve(
        &root.grid,
        &root.elevation_m,
        &root.ocean,
        &root.runoff.local_m3,
    )
    .unwrap();
    root.water_body_owner =
        region_water::root_water_owners(&root.grid, &root.ocean, &root.hydrology).unwrap();
    RegionPlanner::new(&root, identity)
        .prepare(GeoNodeId {
            root: 27,
            level: 4,
            x: 7,
            z: 7,
        })
        .unwrap()
}

#[test]
fn ready_sampling_preserves_published_valley_vertices_without_second_incision() {
    let region = steep_ready_region();
    for z in 0..LATTICE_SIDE {
        for x in 0..LATTICE_SIDE {
            let p = position(region.bounds_m, x, z);
            let expected = &region.columns[z * LATTICE_SIDE + x];
            let actual = region.sample(p[0], p[1]).unwrap();
            assert_eq!(actual, *expected, "vertex {x},{z}");
        }
    }
}

#[test]
fn ready_sampling_reconstructs_interfaces_and_wet_ecology_at_the_actual_coordinate() {
    let region = steep_ready_region();
    let dry = region.sample(-35.9375, -35.9375).unwrap();
    assert!((dry.geology_ecology.geology.bands[4].top_m - dry.ground_m).abs() < 1e-10);
    assert!(
        dry.geology_ecology
            .geology
            .bands
            .windows(2)
            .all(|b| (b[0].top_m - b[1].bottom_m).abs() < 1e-10)
    );
    let bank = region.sample(-32.3, -34.0).unwrap();
    assert_eq!(
        bank.geology_ecology.ecology.wet,
        bank.water_surface_y.is_some()
    );
}

#[test]
fn fractional_macro_resolution_roundoff_preserves_bounded_canonical_edges() {
    let (mut root, identity) = fixture();
    root.grid.min_x_m = 0.0;
    root.grid.min_z_m = 0.0;
    root.grid.width_m = 0.8;
    root.grid.depth_m = 0.8;
    root.grid.step_m = 0.1;
    root.grid.validate().unwrap();
    let planner = RegionPlanner::new(&root, identity);
    let (left, profile) = planner.prepare_cold(node(2)).unwrap();
    let right = planner.prepare(node(3)).unwrap();
    assert!(left.bounds_m[1] - left.bounds_m[0] > root.grid.step_m);
    assert!(profile.candidate_cells <= region_water::MAX_CANDIDATE_CELLS);
    assert_eq!(left.edges[1], right.edges[0]);
    assert!(region_water::prepare(&root, identity, [0.0, 0.1001, 0.0, 0.1]).is_err());
}

#[test]
fn perennial_reach_ecology_is_identical_on_and_between_lattice_coordinates() {
    let region = steep_ready_region();
    let vertex = region.sample(-32.0, -34.0).unwrap();
    let nearby = region.sample(-32.01, -34.003).unwrap();
    for column in [vertex, nearby] {
        let WaterPlan::River { hydroperiod, .. } = &column.water else {
            panic!("fixture point must be inside the perennial reach");
        };
        assert_eq!(hydroperiod.wet_seasons, [true; 4]);
        assert_eq!(column.geology_ecology.ecology.hydroperiod_fraction, 1.0);
        assert_eq!(column.geology_ecology.ecology.moisture_fraction, [1.0; 4]);
        assert_eq!(column.geology_ecology.ecology.water_deficit_mm, [0.0; 4]);
        assert!(column.geology_ecology.ecology.wet);
    }
}
