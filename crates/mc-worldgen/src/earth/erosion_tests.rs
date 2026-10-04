use super::*;

fn grid(columns: usize, rows: usize, step_m: f64) -> Grid {
    Grid {
        columns,
        rows,
        min_x_m: 0.0,
        min_z_m: 0.0,
        width_m: columns as f64 * step_m,
        depth_m: rows as f64 * step_m,
        step_m,
    }
}
fn climate(n: usize) -> ErosionClimate {
    ErosionClimate {
        temperature_c: vec![[15.0; 4]; n],
        precipitation_mm: vec![[0.0; 4]; n],
        potential_et_mm: vec![[0.0; 4]; n],
        snow_fraction: vec![[0.0; 4]; n],
        wind_m_s: vec![[[0.0; 2]; 4]; n],
        ocean: vec![false; n],
    }
}
fn chain(n: usize) -> (Vec<Option<usize>>, Vec<usize>) {
    (
        (0..n).map(|i| (i + 1 < n).then_some(i + 1)).collect(),
        (0..n).map(|i| n - 1 - i).collect(),
    )
}
fn profile(name: &str, grid: &Grid, dem: &[f64], plan: &ErosionPlan) {
    let Some(directory) = std::env::var_os("SOLARIS_W2_ARTIFACT_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut csv = String::from(
        "cell,x_m,z_m,original_m,elevation_m,fluvial_m,glacial_m,hillslope_m,source_coarse_m3,source_fine_m3,deposited_coarse_m3,deposited_fine_m3,transport_coarse_m3,transport_fine_m3,export_coarse_m3,export_fine_m3,stored_coarse_m3,stored_fine_m3,wind_fine_m3,ice_source_m3,ice_ablation_m3,ice_flux_m3,ice_stored_m3,inherited_source_m3,inherited_ablation_m3,inherited_flux_m3,inherited_stored_m3,present_glacier\n",
    );
    for (i, height) in dem.iter().enumerate() {
        let (x, z) = grid.center(i);
        csv.push_str(&format!("{i},{x},{z},{height},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n", plan.elevation_m[i], plan.fluvial_erosion_m[i], plan.glacial_erosion_m[i], plan.hillslope_erosion_m[i], plan.source_m3[i][0], plan.source_m3[i][1], plan.deposited_m3[i][0], plan.deposited_m3[i][1], plan.transported_m3[i][0], plan.transported_m3[i][1], plan.exported_m3[i][0], plan.exported_m3[i][1], plan.stored_m3[i][0], plan.stored_m3[i][1], plan.eolian_transfer_m3[i][1], plan.present_ice.source_m3[i], plan.present_ice.ablation_m3[i], plan.present_ice.transported_m3[i], plan.present_ice.stored_m3[i], plan.inherited_ice.source_m3[i], plan.inherited_ice.ablation_m3[i], plan.inherited_ice.transported_m3[i], plan.inherited_ice.stored_m3[i], plan.present_glacier[i]));
    }
    std::fs::write(directory.join(format!("{name}.csv")), csv).unwrap();
    let mut balance =
        String::from("class,source_m3,deposited_m3,exported_m3,stored_m3,residual_m3\n");
    for c in 0..2 {
        balance.push_str(&format!(
            "{c},{},{},{},{},{}\n",
            plan.source_m3.iter().map(|v| v[c]).sum::<f64>(),
            plan.deposited_m3.iter().map(|v| v[c]).sum::<f64>(),
            plan.exported_m3.iter().map(|v| v[c]).sum::<f64>(),
            plan.stored_m3.iter().map(|v| v[c]).sum::<f64>(),
            plan.sediment_residual_m3[c]
        ));
    }
    std::fs::write(directory.join(format!("{name}-balance.csv")), balance).unwrap();
    let mut ice_csv = String::from(
        "cell,ocean,present_source_m3,present_ablation_m3,present_transport_m3,present_export_or_calving_m3,present_storage_m3,inherited_source_m3,inherited_ablation_m3,inherited_transport_m3,inherited_export_m3,inherited_storage_m3\n",
    );
    for (i, ocean) in plan.ocean.iter().enumerate() {
        ice_csv.push_str(&format!(
            "{i},{ocean},{},{},{},{},{},{},{},{},{},{}\n",
            plan.present_ice.source_m3[i],
            plan.present_ice.ablation_m3[i],
            plan.present_ice.transported_m3[i],
            plan.present_ice.exported_m3[i],
            plan.present_ice.stored_m3[i],
            plan.inherited_ice.source_m3[i],
            plan.inherited_ice.ablation_m3[i],
            plan.inherited_ice.transported_m3[i],
            plan.inherited_ice.exported_m3[i],
            plan.inherited_ice.stored_m3[i],
        ));
    }
    std::fs::write(directory.join(format!("{name}-ice-balance.csv")), ice_csv).unwrap();
    let row = grid.rows / 2;
    let range = row * grid.columns..(row + 1) * grid.columns;
    let min = range
        .clone()
        .map(|i| plan.elevation_m[i].min(dem[i]))
        .fold(f64::INFINITY, f64::min);
    let max = range
        .clone()
        .map(|i| plan.elevation_m[i].max(dem[i]))
        .fold(f64::NEG_INFINITY, f64::max);
    let points = |values: &[f64]| {
        range
            .clone()
            .map(|i| {
                format!(
                    "{},{}",
                    20.0 + (i % grid.columns) as f64 * 560.0
                        / (grid.columns.saturating_sub(1).max(1)) as f64,
                    220.0 - (values[i] - min) * 180.0 / (max - min).max(1.0)
                )
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"600\" height=\"260\"><rect width=\"600\" height=\"260\" fill=\"white\"/><text x=\"20\" y=\"20\">{name}: gray original, blue altered; model meters</text><polyline points=\"{}\" fill=\"none\" stroke=\"gray\"/><polyline points=\"{}\" fill=\"none\" stroke=\"blue\"/></svg>",
        points(dem),
        points(&plan.elevation_m)
    );
    std::fs::write(directory.join(format!("{name}.svg")), svg).unwrap();
}

#[test]
fn e01_soft_material_incises_more_under_identical_forcing() {
    let g = grid(3, 1, 100.0);
    let dem = [30.0, 20.0, 0.0];
    let (down, rank) = chain(3);
    let q = [[2500.0; 4]; 3];
    let c = climate(3);
    let hard = erode(&g, &dem, &[0.15; 3], &[false; 3], (&down, &rank), &q, &c).unwrap();
    let soft = erode(&g, &dem, &[0.8; 3], &[false; 3], (&down, &rank), &q, &c).unwrap();
    assert!(soft.fluvial_erosion_m[0] > 4.0 * hard.fluvial_erosion_m[0]);
    assert!(soft.elevation_m[0] < hard.elevation_m[0]);
    assert!(
        hard.source_m3[0][0] / hard.source_m3[0].iter().sum::<f64>()
            > soft.source_m3[0][0] / soft.source_m3[0].iter().sum::<f64>()
    );
    profile("E01-hard", &g, &dem, &hard);
    profile("E01-soft", &g, &dem, &soft);
}

#[test]
fn e02_solids_balance_through_capacity_loss_and_boundary_export() {
    let mut g = grid(4, 1, 100.0);
    g.width_m = 350.0;
    let dem = [600.0, 400.0, 200.0, 0.0];
    let (down, rank) = chain(4);
    let q = [
        [250_000.0; 4],
        [500_000.0; 4],
        [750_000.0; 4],
        [1_000_000.0; 4],
    ];
    let plan = erode(
        &g,
        &dem,
        &[0.8; 4],
        &[false; 4],
        (&down, &rank),
        &q,
        &climate(4),
    )
    .unwrap();
    for c in 0..2 {
        let source = plan.source_m3.iter().map(|v| v[c]).sum::<f64>();
        let deposit = plan.deposited_m3.iter().map(|v| v[c]).sum::<f64>();
        let export = plan.exported_m3.iter().map(|v| v[c]).sum::<f64>();
        let stored = plan.stored_m3.iter().map(|v| v[c]).sum::<f64>();
        assert!(source > 0.0 && deposit > 0.0 && export > 0.0);
        assert!((source - deposit - export - stored).abs() < 1e-8 * source);
    }
    for i in 0..g.len() {
        assert!(plan.elevation_m[i] >= plan.base_level_m[i]);
        assert!(
            plan.fluvial_erosion_m[i] + plan.glacial_erosion_m[i] + plan.hillslope_erosion_m[i]
                <= MAX_CUT_M
        );
    }
    // Corruption cannot hide behind a still-balanced global sum.
    let mut corrupt = plan.clone();
    corrupt.transported_m3[0][0] += 10.0;
    assert!(corrupt.validate(&g, &dem).is_err());
    profile("E02-material-balance", &g, &dem, &plan);
}

#[test]
fn e03_inherited_mass_flow_broadens_v_valley_without_present_snow() {
    let g = grid(5, 3, 100.0);
    let cross = [30.0, 4.0, 0.0, 4.0, 30.0];
    let dem: Vec<_> = (0..15)
        .map(|i| 450.0 - (i / 5) as f64 * 150.0 + cross[i % 5])
        .collect();
    let down: Vec<_> = (0..15).map(|i| (i + 5 < 15).then_some(i + 5)).collect();
    let rank: Vec<_> = (0..15).map(|i| 14 - i).collect();
    let q: Vec<_> = (0..15)
        .map(|i| {
            if i % 5 == 2 {
                [25_000_000.0; 4]
            } else {
                [0.0; 4]
            }
        })
        .collect();
    let ordinary = erode(
        &g,
        &dem,
        &[0.8; 15],
        &[false; 15],
        (&down, &rank),
        &q,
        &climate(15),
    )
    .unwrap();
    let mask: Vec<_> = (0..15).map(|i| i % 5 == 2 && i < 10).collect();
    let glacial = erode(
        &g,
        &dem,
        &[0.8; 15],
        &mask,
        (&down, &rank),
        &q,
        &climate(15),
    )
    .unwrap();
    let v_gap = (ordinary.elevation_m[6] + ordinary.elevation_m[8]) * 0.5 - ordinary.elevation_m[7];
    let u_gap = (glacial.elevation_m[6] + glacial.elevation_m[8]) * 0.5 - glacial.elevation_m[7];
    assert!(v_gap >= 15.0, "ordinary V gap {v_gap}");
    assert!(u_gap <= 5.0, "broad glacial floor gap {u_gap}");
    assert!(glacial.glacial_erosion_m[6] > 0.0 && glacial.glacial_erosion_m[8] > 0.0);
    assert!(glacial.inherited_ice.transported_m3[7] > 0.0);
    assert!(glacial.inherited_ice.ablation_m3[12] > 0.0);
    assert!(glacial.present_ice.source_m3.iter().all(|v| *v == 0.0));
    assert!(glacial.present_glacier.iter().all(|v| !v));
    profile("E03-ordinary-V", &g, &dem, &ordinary);
    profile("E03-inherited-U", &g, &dem, &glacial);
}

#[test]
fn present_glacier_requires_snow_feed_and_surviving_mass_flux() {
    let g = grid(3, 1, 100.0);
    let dem = [300.0, 150.0, 0.0];
    let (down, rank) = chain(3);
    let mut c = climate(3);
    c.temperature_c.fill([-10.0; 4]);
    let dry = erode(
        &g,
        &dem,
        &[0.8; 3],
        &[false; 3],
        (&down, &rank),
        &[[0.0; 4]; 3],
        &c,
    )
    .unwrap();
    assert!(dry.present_glacier.iter().all(|v| !v));
    assert!(dry.glacial_erosion_m.iter().all(|v| *v == 0.0));
    c.precipitation_mm.fill([500.0; 4]);
    c.snow_fraction.fill([1.0; 4]);
    let fed = erode(
        &g,
        &dem,
        &[0.8; 3],
        &[false; 3],
        (&down, &rank),
        &[[0.0; 4]; 3],
        &c,
    )
    .unwrap();
    assert!(fed.present_glacier[0] && fed.present_glacier[1]);
    assert!(fed.present_ice.transported_m3[0] > 0.0 && fed.glacial_erosion_m[0] > 0.0);
    c.temperature_c.fill([10.0; 4]);
    let melted = erode(
        &g,
        &dem,
        &[0.8; 3],
        &[false; 3],
        (&down, &rank),
        &[[0.0; 4]; 3],
        &c,
    )
    .unwrap();
    assert!(melted.present_glacier.iter().all(|v| !v));
    assert!(melted.present_ice.transported_m3.iter().all(|v| *v == 0.0));
    assert!(close(
        melted.present_ice.source_m3.iter().sum(),
        melted.present_ice.ablation_m3.iter().sum()
    ));
    profile("E03-present-ice", &g, &dem, &fed);
    profile("E03-ablated-ice", &g, &dem, &melted);
}

#[test]
fn wind_moves_only_existing_dry_loose_fines_and_conserves_them() {
    let g = grid(3, 1, 100.0);
    let dem = [100.0, 50.0, 0.0];
    let (down, rank) = chain(3);
    let q = [[25_000.0; 4]; 3];
    let mut c = climate(3);
    c.potential_et_mm.fill([500.0; 4]);
    c.wind_m_s.fill([[20.0, 0.0]; 4]);
    let windy = erode(&g, &dem, &[0.8; 3], &[false; 3], (&down, &rank), &q, &c).unwrap();
    assert!(windy.eolian_transfer_m3[0][1] > 0.0);
    assert_eq!(windy.eolian_receiver[0], Some(1));
    c.precipitation_mm.fill([500.0; 4]);
    let wet = erode(&g, &dem, &[0.8; 3], &[false; 3], (&down, &rank), &q, &c).unwrap();
    assert!(wet.eolian_transfer_m3.iter().all(|v| *v == [0.0; 2]));
    c.precipitation_mm.fill([0.0; 4]);
    let bare = erode(
        &g,
        &[0.0; 3],
        &[0.8; 3],
        &[false; 3],
        (&down, &rank),
        &[[0.0; 4]; 3],
        &c,
    )
    .unwrap();
    assert!(bare.eolian_transfer_m3.iter().all(|v| *v == [0.0; 2]));
    profile("E02-wind-material-balance", &g, &dem, &windy);
}

#[test]
fn closed_sink_stores_or_deposits_solids_but_never_exports() {
    let g = grid(3, 3, 100.0);
    let mut dem = vec![100.0; 9];
    dem[4] = 0.0;
    let mut down = vec![Some(4); 9];
    down[4] = None;
    let mut rank = vec![0; 9];
    let mut next = 1;
    for (i, r) in rank.iter_mut().enumerate() {
        if i != 4 {
            *r = next;
            next += 1;
        }
    }
    let plan = erode(
        &g,
        &dem,
        &[0.8; 9],
        &[false; 9],
        (&down, &rank),
        &[[100_000.0; 4]; 9],
        &climate(9),
    )
    .unwrap();
    assert!(plan.exported_m3.iter().all(|v| *v == [0.0; 2]));
    assert!(plan.deposited_m3[4].iter().sum::<f64>() + plan.stored_m3[4].iter().sum::<f64>() > 0.0);
}

#[test]
fn malformed_graph_climate_and_material_fail_closed() {
    let g = grid(2, 1, 100.0);
    let dem = [10.0, 0.0];
    let q = [[1000.0; 4]; 2];
    let c = climate(2);
    assert!(
        erode(
            &g,
            &dem,
            &[0.5; 2],
            &[false; 2],
            (&[Some(1), Some(0)], &[1, 0]),
            &q,
            &c
        )
        .is_err()
    );
    assert!(
        erode(
            &g,
            &dem,
            &[0.5; 2],
            &[false; 2],
            (&[Some(2), None], &[1, 0]),
            &q,
            &c
        )
        .is_err()
    );
    assert!(
        erode(
            &g,
            &dem,
            &[f64::NAN, 0.5],
            &[false; 2],
            (&[Some(1), None], &[1, 0]),
            &q,
            &c
        )
        .is_err()
    );
    let mut invalid = c;
    invalid.snow_fraction[0][0] = 1.1;
    assert!(
        erode(
            &g,
            &dem,
            &[0.5; 2],
            &[false; 2],
            (&[Some(1), None], &[1, 0]),
            &q,
            &invalid
        )
        .is_err()
    );
}

#[test]
fn talus_threshold_depends_on_material_and_moves_real_solids() {
    let g = grid(2, 1, 100.0);
    let dem = [120.0, 0.0];
    let (down, rank) = chain(2);
    let q = [[0.0; 4]; 2];
    let hard = erode(
        &g,
        &dem,
        &[0.15; 2],
        &[false; 2],
        (&down, &rank),
        &q,
        &climate(2),
    )
    .unwrap();
    let soft = erode(
        &g,
        &dem,
        &[0.8; 2],
        &[false; 2],
        (&down, &rank),
        &q,
        &climate(2),
    )
    .unwrap();
    assert!(soft.hillslope_erosion_m[0] > 5.0 * hard.hillslope_erosion_m[0]);
    assert_eq!(soft.hillslope_receiver[0], Some(1));
    assert!(soft.elevation_m[0] < hard.elevation_m[0]);
    assert!(soft.elevation_m[1] > hard.elevation_m[1]);
    assert!(close(
        soft.source_m3[0].iter().sum(),
        soft.deposited_m3[1].iter().sum()
    ));
    assert!(soft.transported_m3.iter().all(|v| *v == [0.0; 2]));
    profile("E01-soft-talus", &g, &dem, &soft);
    profile("E01-hard-talus", &g, &dem, &hard);
}

#[test]
fn north_south_wind_moves_deposited_fines_in_physical_direction() {
    let g = grid(1, 3, 100.0);
    let dem = [100.0, 50.0, 0.0];
    let (down, rank) = chain(3);
    let q = [[25_000.0; 4]; 3];
    let mut c = climate(3);
    c.potential_et_mm.fill([500.0; 4]);
    c.wind_m_s.fill([[0.0, 20.0]; 4]);
    let north = erode(&g, &dem, &[0.8; 3], &[false; 3], (&down, &rank), &q, &c).unwrap();
    assert!(north.eolian_transfer_m3[1][1] > 0.0);
    assert_eq!(north.eolian_receiver[1], Some(0));
    assert!(g.center(0).1 < g.center(1).1);
    c.wind_m_s.fill([[0.0, -20.0]; 4]);
    let south = erode(&g, &dem, &[0.8; 3], &[false; 3], (&down, &rank), &q, &c).unwrap();
    assert!(south.eolian_transfer_m3[1][1] > 0.0);
    assert_eq!(south.eolian_receiver[1], Some(2));
    assert!(g.center(2).1 > g.center(1).1);
    assert!(north.elevation_m[0] > south.elevation_m[0]);
    assert!(south.elevation_m[2] > north.elevation_m[2]);
    profile("E02-wind-north", &g, &dem, &north);
    profile("E02-wind-south", &g, &dem, &south);
}

#[test]
fn cold_snow_rich_ocean_does_not_create_grounded_ice() {
    let g = grid(3, 1, 100.0);
    let dem = [300.0, 150.0, 0.0];
    let (down, rank) = chain(3);
    let q = [[0.0; 4]; 3];
    let mut c = climate(3);
    c.temperature_c.fill([-10.0; 4]);
    c.precipitation_mm.fill([500.0; 4]);
    c.snow_fraction.fill([1.0; 4]);
    let land = erode(&g, &dem, &[0.8; 3], &[false; 3], (&down, &rank), &q, &c).unwrap();
    assert!(land.present_glacier[0]);
    c.ocean.fill(true);
    let marine = erode(&g, &dem, &[0.8; 3], &[false; 3], (&down, &rank), &q, &c).unwrap();
    assert!(marine.present_ice.source_m3.iter().all(|v| *v == 0.0));
    assert!(marine.present_glacier.iter().all(|v| !v));
    assert!(marine.glacial_erosion_m.iter().all(|v| *v == 0.0));
    // A real terrestrial flow arriving at a marine receiver calves rather
    // than becoming another grounded glacier or continuing across sea floor.
    c.ocean[0] = false;
    let coast = erode(&g, &dem, &[0.8; 3], &[false; 3], (&down, &rank), &q, &c).unwrap();
    assert!(coast.present_ice.transported_m3[0] > 0.0);
    assert!(close(
        coast.present_ice.exported_m3[1],
        coast.present_ice.transported_m3[0]
    ));
    assert_eq!(coast.present_ice.transported_m3[1], 0.0);
    assert_eq!(coast.present_ice.stored_m3[1], 0.0);
    assert!(!coast.present_glacier[1]);
    let mut corrupt = marine.clone();
    corrupt.present_glacier[1] = true;
    assert!(corrupt.validate(&g, &dem).is_err());
    profile("E03-cold-marine-control", &g, &dem, &marine);
    profile("E03-terrestrial-ice-calving", &g, &dem, &coast);
}

#[test]
fn marine_deposited_fines_are_not_a_dry_wind_source() {
    let g = grid(3, 1, 100.0);
    let dem = [100.0, 50.0, 0.0];
    let (down, rank) = chain(3);
    let q = [[25_000.0; 4]; 3];
    let mut c = climate(3);
    c.potential_et_mm.fill([500.0; 4]);
    c.wind_m_s.fill([[20.0, 0.0]; 4]);
    let land = erode(&g, &dem, &[0.8; 3], &[false; 3], (&down, &rank), &q, &c).unwrap();
    assert!(land.deposited_m3[0][1] > 0.0 && land.eolian_transfer_m3[0][1] > 0.0);
    c.ocean.fill(true);
    let marine = erode(&g, &dem, &[0.8; 3], &[false; 3], (&down, &rank), &q, &c).unwrap();
    assert!(marine.deposited_m3[0][1] > 0.0);
    assert!(marine.eolian_transfer_m3.iter().all(|v| *v == [0.0; 2]));
    // Wind may export terrestrial loose fines into the ocean receiver.
    c.ocean[0] = false;
    let coast = erode(&g, &dem, &[0.8; 3], &[false; 3], (&down, &rank), &q, &c).unwrap();
    assert!(coast.eolian_transfer_m3[0][1] > 0.0);
    assert_eq!(coast.eolian_receiver[0], Some(1));
    assert_eq!(coast.eolian_transfer_m3[1], [0.0; 2]);
    assert!(coast.deposited_m3[1][1] > marine.deposited_m3[1][1]);
    profile("E02-marine-fines-wind-control", &g, &dem, &marine);
    profile("E02-land-wind-marine-deposition", &g, &dem, &coast);
}
