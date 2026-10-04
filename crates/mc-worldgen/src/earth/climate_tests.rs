use super::*;

fn grid(columns: usize, rows: usize, step: f64) -> Grid {
    Grid {
        columns,
        rows,
        min_x_m: 0.0,
        min_z_m: 0.0,
        width_m: columns as f64 * step,
        depth_m: rows as f64 * step,
        step_m: step,
    }
}

fn ridge() -> (Grid, Vec<f64>, Vec<bool>) {
    let grid = grid(25, 3, 20_000.0);
    let mut dem = vec![0.0; grid.len()];
    let mut ocean = vec![false; grid.len()];
    for i in 0..grid.len() {
        let x = i % grid.columns;
        dem[i] = match x {
            10 | 14 => 800.0,
            11 | 13 => 1800.0,
            12 => 3000.0,
            _ => 0.0,
        };
        ocean[i] = !(4..=20).contains(&x);
        if ocean[i] {
            dem[i] = -1000.0;
        }
    }
    (grid, dem, ocean)
}

fn mean_rain(plan: &ClimatePlan, grid: &Grid, from: usize, to: usize) -> f64 {
    let mut volume = 0.0;
    let mut area = 0.0;
    for (i, cell) in plan.cells.iter().enumerate() {
        if (from..=to).contains(&(i % grid.columns)) {
            area += grid.area(i);
            volume += cell.annual_precipitation_mm() * grid.area(i);
        }
    }
    volume / area
}

/// Optional artifacts are numerical profiles plus a deterministic SVG map; no
/// production file writes and no dependency on the current working directory.
fn profile(name: &str, grid: &Grid, dem: &[f64], plan: &ClimatePlan) {
    let Some(directory) = std::env::var_os("SOLARIS_W2_ARTIFACT_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut csv = String::from(
        "cell,x_m,z_m,area_m2,height_m,ocean,annual_temperature_c,annual_precipitation_mm,t0_c,t1_c,t2_c,t3_c,p0_mm,p1_mm,p2_mm,p3_mm,sst0_c,east0_m_s,north0_m_s,evap0_mm\n",
    );
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\"><title>{name}: annual precipitation, blue=wet red=dry</title>",
        grid.columns * 12,
        grid.rows * 12
    );
    use std::fmt::Write as _;
    for (i, cell) in plan.cells.iter().enumerate() {
        let (x, z) = grid.center(i);
        writeln!(
            csv,
            "{i},{x},{z},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            grid.area(i),
            dem[i],
            cell.ocean,
            cell.annual_temperature_c(),
            cell.annual_precipitation_mm(),
            cell.temperature_c[0],
            cell.temperature_c[1],
            cell.temperature_c[2],
            cell.temperature_c[3],
            cell.precipitation_mm[0],
            cell.precipitation_mm[1],
            cell.precipitation_mm[2],
            cell.precipitation_mm[3],
            cell.sst_c[0],
            cell.wind_m_s[0][0],
            cell.wind_m_s[0][1],
            cell.evaporation_mm[0]
        )
        .unwrap();
        let blue = (cell.annual_precipitation_mm() / 1500.0 * 255.0).clamp(0.0, 255.0) as u8;
        write!(svg, "<rect x=\"{}\" y=\"{}\" width=\"12\" height=\"12\" fill=\"rgb({},40,{blue})\"><title>cell {i}; height {}; T {}; P {}; SST {}</title></rect>", (i % grid.columns) * 12, (i / grid.columns) * 12, 255 - blue, dem[i], cell.annual_temperature_c(), cell.annual_precipitation_mm(), cell.sst_c[0]).unwrap();
    }
    svg.push_str("</svg>\n");
    std::fs::write(directory.join(format!("{name}.csv")), csv).unwrap();
    std::fs::write(directory.join(format!("{name}.svg")), svg).unwrap();
    let mut budgets = String::from(
        "season,evaporation_m3,inflow_m3,initial_storage_m3,precipitation_m3,outflow_m3,final_storage_m3,cfl,timestep_s,heat_initial_degree_m2,heat_relaxation_degree_m2,heat_final_degree_m2\n",
    );
    for season in 0..4 {
        let b = &plan.moisture_budgets[season];
        let h = &plan.ocean_heat_budgets[season];
        writeln!(
            budgets,
            "{season},{},{},{},{},{},{},{},{},{},{},{}",
            b.evaporation_m3,
            b.external_inflow_m3,
            b.initial_storage_m3,
            b.precipitation_m3,
            b.external_outflow_m3,
            b.final_storage_m3,
            b.maximum_cfl,
            b.timestep_s,
            h.initial_degree_m2,
            h.relaxation_degree_m2,
            h.final_degree_m2
        )
        .unwrap();
    }
    std::fs::write(directory.join(format!("{name}-budgets.csv")), budgets).unwrap();
}

#[test]
fn c01_ocean_upwind_ridge_has_rain_and_a_downstream_deficit() {
    let (grid, dem, mut ocean) = ridge();
    for (i, marine) in ocean.iter_mut().enumerate() {
        if i % grid.columns > 20 {
            *marine = false;
        }
    }
    let mut land_dem = dem.clone();
    for i in 0..grid.len() {
        if !ocean[i] && land_dem[i] < 0.0 {
            land_dem[i] = 0.0;
        }
    }
    let plan = solve_inner(
        &grid,
        &land_dem,
        &ocean,
        30_000.0,
        3_000_000.0,
        Some([8.0, 0.0]),
        true,
    )
    .unwrap();
    let mut flat = land_dem.clone();
    for value in &mut flat {
        if *value > 0.0 {
            *value = 0.0;
        }
    }
    let reference = solve_inner(
        &grid,
        &flat,
        &ocean,
        30_000.0,
        3_000_000.0,
        Some([8.0, 0.0]),
        true,
    )
    .unwrap();
    assert!(mean_rain(&plan, &grid, 10, 12) > mean_rain(&plan, &grid, 16, 19) * 2.0);
    assert!(mean_rain(&plan, &grid, 10, 12) > mean_rain(&reference, &grid, 10, 12));
    assert!(mean_rain(&plan, &grid, 16, 19) < mean_rain(&reference, &grid, 16, 19));
    profile("C01-ridge", &grid, &land_dem, &plan);
    profile("C01-flat", &grid, &flat, &reference);
}

#[test]
fn c02_reversing_wind_reverses_wet_and_dry_sides() {
    let (grid, dem, ocean) = ridge();
    let east = solve_inner(
        &grid,
        &dem,
        &ocean,
        30_000.0,
        3_000_000.0,
        Some([8.0, 0.0]),
        false,
    )
    .unwrap();
    let west = solve_inner(
        &grid,
        &dem,
        &ocean,
        30_000.0,
        3_000_000.0,
        Some([-8.0, 0.0]),
        false,
    )
    .unwrap();
    assert!(mean_rain(&east, &grid, 10, 12) > mean_rain(&east, &grid, 14, 16));
    assert!(mean_rain(&west, &grid, 12, 14) > mean_rain(&west, &grid, 8, 10));
    for row in 0..grid.rows {
        for x in 0..grid.columns {
            let a = east.cells[row * grid.columns + x].annual_precipitation_mm();
            let b = west.cells[row * grid.columns + grid.columns - 1 - x].annual_precipitation_mm();
            assert!(
                (a - b).abs() < 1e-8,
                "reversal must reflect the full moisture profile: {a} {b}"
            );
        }
    }
    profile("C02-east", &grid, &dem, &east);
    profile("C02-west", &grid, &dem, &west);
}

#[test]
fn c03_same_latitude_physical_lapse_is_six_degrees_per_kilometre() {
    let grid = grid(3, 1, 1000.0);
    let dem = [0.0, 1000.0, 3000.0];
    let plan = solve(&grid, &dem, &[false; 3], 500.0, 3_000_000.0).unwrap();
    for season in 0..4 {
        assert!(
            (plan.cells[0].temperature_c[season] - plan.cells[1].temperature_c[season] - 6.0).abs()
                < 1e-12
        );
        assert!(
            (plan.cells[0].temperature_c[season] - plan.cells[2].temperature_c[season] - 18.0)
                .abs()
                < 1e-12
        );
    }
    profile("C03-physical-lapse", &grid, &dem, &plan);
    // C04 is API isolation evidence only: native metric runtime mapping remains
    // gated by §32. Compare reference renderings of this physical profile without
    // feeding either rendering into climate. Compact formula is §4.4.
    if let Some(directory) = std::env::var_os("SOLARIS_W2_ARTIFACT_DIR") {
        use std::fmt::Write as _;
        let mut csv =
            String::from("physical_height_m,compact_y,metric_reference_y,temperature_c\n");
        for (i, height) in dem.iter().enumerate() {
            let compact = 63.0 + 224.0 * (height / 3000.0).tanh();
            writeln!(
                csv,
                "{height},{},{},{}",
                compact.round(),
                height.round(),
                plan.cells[i].annual_temperature_c()
            )
            .unwrap();
        }
        std::fs::write(
            std::path::PathBuf::from(directory).join("C04-mapping-isolation-only.csv"),
            csv,
        )
        .unwrap();
    }
}

#[test]
fn c05_closing_strait_blocks_heat_and_disabling_transport_removes_anomaly() {
    let grid = grid(5, 9, 100_000.0);
    let mut dem = vec![20.0; grid.len()];
    let mut ocean = vec![false; grid.len()];
    for i in 0..grid.len() {
        if (1..=3).contains(&(i % grid.columns)) {
            ocean[i] = true;
            dem[i] = -1000.0;
        }
    }
    let equator = 850_000.0;
    let radius = 700_000.0;
    let open = solve_inner(&grid, &dem, &ocean, equator, radius, None, true).unwrap();
    let disabled = solve_inner(&grid, &dem, &ocean, equator, radius, None, false).unwrap();
    let mut closed_ocean = ocean.clone();
    let mut closed_dem = dem.clone();
    for x in 1..=3 {
        closed_ocean[grid.columns + x] = false;
        closed_dem[grid.columns + x] = 20.0;
    }
    let closed = solve_inner(
        &grid,
        &closed_dem,
        &closed_ocean,
        equator,
        radius,
        None,
        true,
    )
    .unwrap();
    let target = 1;
    assert!(open.cells[target].sst_c[0] > disabled.cells[target].sst_c[0] + 0.1);
    assert!((closed.cells[target].sst_c[0] - disabled.cells[target].sst_c[0]).abs() < 1e-10);
    assert!(open.cells[0].temperature_c[0] > disabled.cells[0].temperature_c[0] + 0.05);
    assert!(open.cells[target].evaporation_mm[0] > disabled.cells[target].evaporation_mm[0]);
    let mut divergence = vec![0.0; grid.len()];
    for edge in &open.ocean_edges {
        assert!(ocean[edge.from] && ocean[edge.to]);
        let (w, d) = dimensions(&grid, edge.from);
        let length = if edge.to == edge.from + 1 { d } else { w };
        divergence[edge.from] += edge.velocity_m_s * length;
        divergence[edge.to] -= edge.velocity_m_s * length;
    }
    assert!(divergence.into_iter().all(|v| v.abs() < 1e-8));
    assert!(
        closed
            .ocean_edges
            .iter()
            .all(|e| closed_ocean[e.from] && closed_ocean[e.to])
    );
    profile("C05-open", &grid, &dem, &open);
    profile("C05-disabled", &grid, &dem, &disabled);
    profile("C05-closed", &grid, &closed_dem, &closed);
}

#[test]
fn ocean_seeds_flood_shelves_but_not_disconnected_continental_pits_or_boundary() {
    let grid = grid(5, 3, 1000.0);
    let mut dem = vec![100.0; grid.len()];
    let mut oceanic = vec![false; grid.len()];
    for i in [0, 1, 5, 6] {
        dem[i] = -1000.0;
    }
    oceanic[0] = true;
    dem[4] = -5000.0; // A boundary is not an external reservoir.
    dem[13] = -5000.0; // A dry continental basin below sea level.
    oceanic[9] = true; // Oceanic crust above sea level is dry.
    let (mask, seeds) = ocean_mask_with_seeds(&grid, &dem, &oceanic).unwrap();
    assert_eq!(seeds, vec![0]);
    assert_eq!(
        mask,
        (0..grid.len())
            .map(|i| [0, 1, 5, 6].contains(&i))
            .collect::<Vec<_>>()
    );
    let plan = solve(&grid, &dem, &mask, 1500.0, 300_000.0).unwrap();
    profile("ocean-connectivity", &grid, &dem, &plan);
}

#[test]
fn ocean_seed_selection_uses_partial_area_and_row_major_ties() {
    let grid = Grid {
        columns: 3,
        rows: 1,
        min_x_m: 0.0,
        min_z_m: 0.0,
        width_m: 2100.0,
        depth_m: 1000.0,
        step_m: 1000.0,
    };
    // The deepest cell is only a tenth-cell wide; the equally ranked full cells
    // tie and select the lower stable ID, not the deepest cell or discovery order.
    let (mask, seeds) =
        ocean_mask_with_seeds(&grid, &[-100.0, -100.0, -500.0], &[true; 3]).unwrap();
    assert_eq!(mask, vec![true; 3]);
    assert_eq!(seeds, vec![0]);
}

#[test]
fn partial_cells_conserve_seasonal_water_and_corruption_is_rejected() {
    let grid = Grid {
        columns: 4,
        rows: 3,
        min_x_m: -1000.0,
        min_z_m: 2000.0,
        width_m: 3200.0,
        depth_m: 2150.0,
        step_m: 1000.0,
    };
    let dem = vec![-1000.0; grid.len()];
    let ocean = vec![true; grid.len()];
    let plan = solve(&grid, &dem, &ocean, 3000.0, 20_000.0).unwrap();
    for b in &plan.moisture_budgets {
        let source = b.evaporation_m3 + b.external_inflow_m3 + b.initial_storage_m3;
        let sink = b.precipitation_m3 + b.external_outflow_m3 + b.final_storage_m3;
        assert!((source - sink).abs() <= source * 1e-12);
        assert!(b.maximum_cfl <= CFL + 1e-12);
    }
    assert_eq!(plan, solve(&grid, &dem, &ocean, 3000.0, 20_000.0).unwrap());
    let mut corrupt = plan.clone();
    corrupt.cells[0].precipitation_mm[0] += 10.0;
    assert!(corrupt.validate(&grid).is_err());
    let mut corrupt = plan.clone();
    corrupt.moisture_budgets[0].timestep_s *= 10.0;
    assert!(corrupt.validate(&grid).is_err());
    let mut corrupt = plan.clone();
    corrupt.ocean_edges[0].to = grid.len();
    assert!(corrupt.validate(&grid).is_err());
    let mut corrupt = plan.clone();
    corrupt.atmosphere_boundary[0].inflow_m3[0] += 10.0;
    assert!(corrupt.validate(&grid).is_err());
    assert!(solve(&grid, &dem[..1], &ocean, 3000.0, 20_000.0).is_err());
    assert!(solve(&grid, &dem, &ocean, f64::NAN, 20_000.0).is_err());
    assert!(solve(&grid, &dem, &ocean, 3000.0, 0.0).is_err());
    let mut invalid = dem.clone();
    invalid[0] = f64::INFINITY;
    assert!(ocean_mask(&grid, &invalid, &ocean).is_err());
    profile("partial-cell-budgets", &grid, &dem, &plan);
}

#[test]
fn dry_subsea_height_warms_air_without_becoming_marine() {
    let g = grid(2, 1, 50_000.0);
    let dem = [-1000.0, 0.0];
    let plan = solve(&g, &dem, &[false; 2], 0.0, 6_371_000.0).unwrap();
    for s in 0..4 {
        assert!(
            (plan.cells[0].temperature_c[s] - plan.cells[1].temperature_c[s] - 6.0).abs() < 1e-12
        );
    }
    assert!(!plan.cells[0].ocean);
    profile("C03-dry-subsea-lapse", &g, &dem, &plan);
}

#[test]
fn submerged_bathymetry_does_not_create_atmospheric_orography() {
    let g = grid(5, 3, 20_000.0);
    let ocean = vec![true; g.len()];
    let flat = vec![-1000.0; g.len()];
    let relief: Vec<_> = (0..g.len())
        .map(|i| if i % g.columns == 2 { -50.0 } else { -6000.0 })
        .collect();
    let flat_plan = solve(&g, &flat, &ocean, 0.0, 6_371_000.0).unwrap();
    let relief_plan = solve(&g, &relief, &ocean, 0.0, 6_371_000.0).unwrap();
    assert_eq!(relief_plan, flat_plan);
    profile(
        "C03-ocean-surface-not-bathymetry",
        &g,
        &relief,
        &relief_plan,
    );
}
