use super::*;

fn grid(columns: usize, rows: usize) -> Grid {
    Grid {
        columns,
        rows,
        min_x_m: 0.0,
        min_z_m: 0.0,
        width_m: columns as f64,
        depth_m: rows as f64,
        step_m: 1.0,
    }
}
fn fixture(
    name: &str,
    grid: &Grid,
    dem: &[f64],
    ocean: &[bool],
    input: &[[f64; 4]],
) -> HydrologyPlan {
    let plan = solve(grid, dem, ocean, input).unwrap();
    profile(name, grid, dem, &plan);
    plan
}
fn profile(name: &str, grid: &Grid, dem: &[f64], p: &HydrologyPlan) {
    let Some(directory) = std::env::var_os("SOLARIS_W2_ARTIFACT_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut csv = String::from(
        "cell,x_m,z_m,dem_m,downstream,rank,lake_m,volume_s0_m3,volume_s1_m3,volume_s2_m3,volume_s3_m3,lake_s0_m,lake_s1_m,lake_s2_m,lake_s3_m\n",
    );
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\">",
        grid.columns * 60,
        grid.rows * 60
    );
    for (i, h) in dem.iter().enumerate() {
        let (x, z) = grid.center(i);
        csv.push_str(&format!(
            "{i},{x},{z},{h},{:?},{},{:?},{},{},{},{},{:?},{:?},{:?},{:?}\n",
            p.downstream[i],
            p.rank[i],
            p.water_level_m[i],
            p.discharge_m3[i][0],
            p.discharge_m3[i][1],
            p.discharge_m3[i][2],
            p.discharge_m3[i][3],
            p.seasonal_water_level_m[i][0],
            p.seasonal_water_level_m[i][1],
            p.seasonal_water_level_m[i][2],
            p.seasonal_water_level_m[i][3]
        ));
        let fill = if p.water_level_m[i].is_some() {
            "#83bbdf"
        } else {
            "#d9ccaa"
        };
        svg.push_str(&format!("<rect x=\"{}\" y=\"{}\" width=\"60\" height=\"60\" fill=\"{fill}\" stroke=\"#555\"/><text x=\"{}\" y=\"{}\" font-size=\"11\">{i}: {h}</text>",i%grid.columns*60,i/grid.columns*60,i%grid.columns*60+3,i/grid.columns*60+15));
        if let Some(j) = p.downstream[i] {
            svg.push_str(&format!(
                "<path d=\"M {} {} L {} {}\" stroke=\"#247\" stroke-width=\"2\"/>",
                i % grid.columns * 60 + 30,
                i / grid.columns * 60 + 30,
                j % grid.columns * 60 + 30,
                j / grid.columns * 60 + 30
            ));
        }
    }
    svg.push_str("</svg>");
    std::fs::write(directory.join(format!("{name}.csv")), csv).unwrap();
    std::fs::write(directory.join(format!("{name}.svg")), svg).unwrap();
    let mut lakes = String::from(
        "id,parent,season,minimum_m,spill_m,capacity_m3,inflow_m3,storage_m3,outflow_m3,evaporation_m3,groundwater_export_m3,level_m\n",
    );
    for d in &p.depressions {
        for s in 0..4 {
            lakes.push_str(&format!(
                "{},{:?},{s},{},{},{},{},{},{},{},{},{:?}\n",
                d.id,
                d.parent,
                d.minimum_m,
                d.spill_level_m,
                d.capacity_m3,
                d.inflow_m3[s],
                d.storage_m3[s],
                d.outflow_m3[s],
                d.evaporation_m3[s],
                d.groundwater_export_m3[s],
                d.level_m[s]
            ));
        }
    }
    std::fs::write(directory.join(format!("{name}_lakes.csv")), lakes).unwrap();
}
fn bowl(base: f64) -> (Grid, Vec<f64>, Vec<bool>, Vec<[f64; 4]>) {
    let g = grid(5, 5);
    let mut dem = vec![base + 10.0; g.len()];
    dem[12] = base;
    for i in [7, 11, 13, 17] {
        dem[i] = base + 2.0;
    }
    let ocean = vec![false; g.len()];
    let input = vec![[0.0; 4]; g.len()];
    (g, dem, ocean, input)
}
#[test]
fn h01_dem_slope_accumulates_all_upstream() {
    let g = grid(8, 1);
    let dem: Vec<_> = (0..8).map(|i| 7.0 - i as f64).collect();
    let mut ocean = vec![false; 8];
    ocean[7] = true;
    let input = vec![[1.0, 2.0, 3.0, 4.0]; 8];
    let p = fixture("H01", &g, &dem, &ocean, &input);
    for i in 0..7 {
        assert_eq!(p.downstream[i], Some(i + 1));
        assert_eq!(p.discharge_m3[i][0], (i + 1) as f64);
    }
    assert_eq!(p.ocean_outflow_m3, [8.0, 16.0, 24.0, 32.0]);
    assert!(close(
        p.annual_mean_q_m3_s(7),
        80.0 / SECONDS_PER_MODEL_YEAR
    ));
}
#[test]
fn h02_three_tributaries_share_a_real_confluence() {
    let g = grid(3, 3);
    let dem = vec![9.0, 3.0, 9.0, 3.0, 2.0, 3.0, 9.0, 1.0, 9.0];
    let mut ocean = vec![false; 9];
    ocean[7] = true;
    let mut input = vec![[0.0; 4]; 9];
    for i in [1, 3, 5] {
        input[i] = [10.0; 4];
    }
    let p = fixture("H02", &g, &dem, &ocean, &input);
    for i in [1, 3, 5] {
        assert_eq!(p.downstream[i], Some(4));
    }
    assert_eq!(p.discharge_m3[4], [30.0; 4]);
    assert_eq!(p.downstream[4], Some(7));
}
#[test]
fn h03_wet_basin_rises_then_spills_at_its_saddle() {
    let (g, dem, ocean, mut input) = bowl(0.0);
    input[12] = [20.0; 4];
    let p = fixture("H03", &g, &dem, &ocean, &input);
    let r = p.depression[12].unwrap();
    assert_eq!(p.depressions[r].capacity_m3, 42.0);
    assert!(close(p.seasonal_water_level_m[12][0].unwrap(), 5.6));
    assert!(close(p.seasonal_water_level_m[12][1].unwrap(), 9.6));
    assert_eq!(p.water_level_m[12], Some(10.0));
    assert_eq!(p.depressions[r].outflow_m3, [0.0, 0.0, 18.0, 20.0]);
}
#[test]
fn h04_subsea_dry_basin_is_not_an_ocean() {
    let (g, dem, ocean, input) = bowl(-30.0);
    let p = fixture("H04", &g, &dem, &ocean, &input);
    assert_eq!(p.water_level_m[12], None);
    assert!(p.ocean_outlets.is_empty());
    assert_eq!(p.ocean_outflow_m3, [0.0; 4]);
    assert!(p.depression[12].is_some());
}
#[test]
fn h05_nested_depressions_fill_spill_and_merge_causally() {
    let g = grid(7, 5);
    let mut dem = vec![10.0; g.len()];
    for (i, h) in [(15, 0.0), (16, 1.0), (17, 4.0), (18, 1.0), (19, 0.0)] {
        dem[i] = h;
    }
    let ocean = vec![false; g.len()];
    let mut input = vec![[0.0; 4]; g.len()];
    input[15] = [5.0, 10.0, 40.0, 0.0];
    let p = fixture("H05", &g, &dem, &ocean, &input);
    assert_eq!(p.seasonal_water_level_m[19][0], None);
    assert_eq!(p.seasonal_water_level_m[15][0], Some(3.0));
    assert_eq!(
        p.seasonal_water_level_m[15][1],
        p.seasonal_water_level_m[19][1]
    );
    assert!(close(p.seasonal_water_level_m[15][1].unwrap(), 4.2));
    let root = p.depression[15].unwrap();
    assert_eq!(p.depression[19], Some(root));
    assert_eq!(p.depressions[root].children.len(), 2);
    assert_eq!(p.depressions[root].capacity_m3, 44.0);
    assert_eq!(p.depressions[root].outflow_m3, [0.0, 0.0, 11.0, 0.0]);
    // Identical terrain, alternate physical source: the unfilled basin changes,
    // but merged levels and total water do not depend on fixture coordinates.
    input[15] = [0.0; 4];
    input[19] = [5.0, 10.0, 40.0, 0.0];
    let mirrored = fixture("H05_alternate_source", &g, &dem, &ocean, &input);
    assert_eq!(mirrored.seasonal_water_level_m[15][0], None);
    assert_eq!(mirrored.seasonal_water_level_m[19][0], Some(3.0));
    assert_eq!(mirrored.water_level_m[15], p.water_level_m[15]);
}
#[test]
fn h06_explained_channel_losses_reduce_only_available_water() {
    let g = grid(5, 1);
    let dem = vec![4.0, 3.0, 2.0, 1.0, 0.0];
    let ocean = vec![false; 5];
    let mut input = vec![[0.0; 4]; 5];
    input[0] = [20.0, 2.0, 0.0, 20.0];
    let mut losses = ChannelLosses {
        evaporation_m3: vec![[0.0; 4]; 5],
        groundwater_export_m3: vec![[0.0; 4]; 5],
    };
    losses.evaporation_m3[2] = [3.0; 4];
    losses.groundwater_export_m3[2] = [2.0; 4];
    let p = solve_with_losses(&g, &dem, &ocean, &input, &losses).unwrap();
    profile("H06", &g, &dem, &p);
    assert_eq!(p.discharge_m3[1], input[0]);
    assert_eq!(p.discharge_m3[2], [15.0, 0.0, 0.0, 15.0]);
    assert_eq!(p.evaporation_m3[2], [3.0, 2.0, 0.0, 3.0]);
    assert_eq!(p.groundwater_export_m3[2], [2.0, 0.0, 0.0, 2.0]);
}
#[test]
fn h07_snow_soil_groundwater_and_dry_season_close_the_balance() {
    let g = grid(1, 1);
    let p = runoff(
        &g,
        &[[-5.0, 5.0, 20.0, 10.0]],
        &[[1000.0, 0.0, 0.0, 0.0]],
        &[[0.0, 0.0, 20.0, 5000.0]],
        &[[1.0, 0.0, 0.0, 0.0]],
        &[false],
    )
    .unwrap();
    assert_eq!(p.local_m3[0][0], 0.0);
    assert_eq!(p.snow_storage_m3[0][0], 1.0);
    assert_eq!(p.snowmelt_m3[0][1], 0.45);
    assert!(p.local_m3[0][1] > 0.0);
    assert!(p.baseflow_m3[0][3] > 0.0);
    assert!(p.actual_et_m3[0][3] < 5.0);
    let hydro = fixture("H07", &g, &[10.0], &[false], &p.local_m3);
    assert_eq!(hydro.external_outflow_m3, p.local_m3[0]);
    if let Some(path) = std::env::var_os("SOLARIS_W2_ARTIFACT_DIR") {
        let mut csv = String::from(
            "season,precipitation_m3,runoff_m3,et_m3,snow_m3,soil_m3,groundwater_m3\n",
        );
        for s in 0..4 {
            csv.push_str(&format!(
                "{s},{},{},{},{},{},{}\n",
                p.precipitation_m3[0][s],
                p.local_m3[0][s],
                p.actual_et_m3[0][s],
                p.snow_storage_m3[0][s],
                p.soil_storage_m3[0][s],
                p.groundwater_storage_m3[0][s]
            ));
        }
        std::fs::write(
            std::path::PathBuf::from(path).join("H07_reservoirs.csv"),
            csv,
        )
        .unwrap();
    }
}
#[test]
fn h08_flats_use_rank_not_epsilon_dem() {
    let g = grid(6, 6);
    let dem = vec![100.0; g.len()];
    let input = vec![[1.0; 4]; g.len()];
    let ocean = vec![false; g.len()];
    let p = fixture("H08", &g, &dem, &ocean, &input);
    for i in 0..g.len() {
        if let Some(j) = p.downstream[i] {
            assert!(p.rank[j] < p.rank[i]);
            assert_eq!(dem[i], dem[j]);
        }
    }
    assert_eq!(p.external_outflow_m3, [36.0; 4]);
    assert!(p.water_level_m.iter().all(Option::is_none));
}
#[test]
fn h09_upland_lake_has_its_own_surface_and_outlet() {
    let (g, dem, ocean, mut input) = bowl(1000.0);
    input[12] = [100.0; 4];
    let p = fixture("H09", &g, &dem, &ocean, &input);
    assert_eq!(p.water_level_m[12], Some(1010.0));
    assert_eq!(p.external_outflow_m3, [58.0, 100.0, 100.0, 100.0]);
    assert_eq!(
        p.depressions[p.depression[12].unwrap()].spill_level_m,
        1010.0
    );
}
#[test]
fn h10_mountain_to_ocean_trace_preserves_real_profile() {
    let g = grid(7, 1);
    let dem = vec![3000.0, 2400.0, 1800.0, 900.0, 300.0, 10.0, -20.0];
    let original = dem.clone();
    let mut ocean = vec![false; 7];
    ocean[6] = true;
    let mut input = vec![[0.0; 4]; 7];
    input[0] = [25.0; 4];
    let p = fixture("H10", &g, &dem, &ocean, &input);
    let mut i = 0;
    while let Some(j) = p.downstream[i] {
        assert!(dem[j] < dem[i]);
        assert_eq!(p.discharge_m3[i], [25.0; 4]);
        i = j;
    }
    assert_eq!(i, 6);
    assert_eq!(dem, original);
    assert_eq!(p.ocean_outflow_m3, [25.0; 4]);
}
#[test]
fn h11_distributaries_split_one_flow_instead_of_copying_it() {
    let g = grid(5, 1);
    let dem = vec![4.0, 3.0, 2.0, 1.0, 0.0];
    let mut input = vec![[0.0; 4]; 5];
    input[0] = [100.0, 50.0, 0.0, 200.0];
    let p = fixture("H11", &g, &dem, &[false; 5], &input);
    let delta = split_delta(&p, 2, &[3, 4], &[0.25, 0.75]).unwrap();
    assert_eq!(delta.discharge_m3[0], [25.0, 12.5, 0.0, 50.0]);
    for s in 0..4 {
        assert_eq!(
            delta.discharge_m3.iter().map(|v| v[s]).sum::<f64>(),
            p.discharge_m3[2][s]
        );
    }
    assert!(split_delta(&p, 2, &[3, 4], &[1.0, 1.0]).is_err());
    if let Some(directory) = std::env::var_os("SOLARIS_W2_ARTIFACT_DIR") {
        let mut csv = String::from(
            "source,receiver,fraction,volume_s0_m3,volume_s1_m3,volume_s2_m3,volume_s3_m3\n",
        );
        for i in 0..delta.receiving_cells.len() {
            csv.push_str(&format!(
                "{},{},{},{},{},{},{}\n",
                delta.source_cell,
                delta.receiving_cells[i],
                delta.fractions[i],
                delta.discharge_m3[i][0],
                delta.discharge_m3[i][1],
                delta.discharge_m3[i][2],
                delta.discharge_m3[i][3]
            ));
        }
        std::fs::write(
            std::path::PathBuf::from(directory).join("H11_delta.csv"),
            csv,
        )
        .unwrap();
    }
}
#[test]
fn validation_rejects_cycles_unknown_ids_and_hidden_water() {
    let g = grid(3, 1);
    let dem = [2.0, 1.0, 0.0];
    let input = [[1.0; 4]; 3];
    let ocean = [false; 3];
    let p = solve(&g, &dem, &ocean, &input).unwrap();
    let mut corrupt = p.clone();
    corrupt.downstream[2] = Some(1);
    assert!(corrupt.validate(&g, &dem, &ocean, &input).is_err());
    let mut corrupt = p.clone();
    corrupt.depression[0] = Some(usize::MAX);
    assert!(corrupt.validate(&g, &dem, &ocean, &input).is_err());
    let mut corrupt = p;
    corrupt.external_outflow_m3[0] += 1.0;
    assert!(corrupt.validate(&g, &dem, &ocean, &input).is_err());
}

#[test]
fn lake_pet_drains_stored_water_and_dry_demand_cannot_create_a_loss() {
    let (g, dem, ocean, mut input) = bowl(100.0);
    input[12] = [20.0, 0.0, 0.0, 0.0];
    let mut losses = LakeLosses {
        evaporation_demand_m3: vec![[0.0; 4]; g.len()],
        groundwater_export_demand_m3: vec![[0.0; 4]; g.len()],
    };
    losses.evaporation_demand_m3[12] = [0.0, 5.0, 100.0, 100.0];
    losses.groundwater_export_demand_m3[12] = [0.0, 3.0, 0.0, 100.0];
    // Dry high ground has demand too, but is not an evaporating lake mirror.
    losses.evaporation_demand_m3[6] = [1000.0; 4];
    let p = solve_with_lake_losses(&g, &dem, &ocean, &input, &losses).unwrap();
    profile("H03_lake_pet", &g, &dem, &p);
    assert_eq!(p.seasonal_storage_m3(), [20.0, 12.0, 0.0, 0.0]);
    assert_eq!(p.lake_evaporation_m3[12], [0.0, 5.0, 12.0, 0.0]);
    assert_eq!(p.lake_groundwater_export_m3[12], [0.0, 3.0, 0.0, 0.0]);
    assert_eq!(p.lake_evaporation_m3[6], [0.0; 4]);
    assert_eq!(p.water_level_m[12], None);
    assert_eq!(p.external_outflow_m3, [0.0; 4]);
}

#[test]
fn merged_lake_evaporation_separates_basins_without_stealing_sibling_stock() {
    let g = grid(7, 5);
    let mut dem = vec![10.0; g.len()];
    for (i, h) in [(15, 0.0), (16, 1.0), (17, 4.0), (18, 1.0), (19, 0.0)] {
        dem[i] = h;
    }
    let mut input = vec![[0.0; 4]; g.len()];
    input[15] = [20.0, 0.0, 0.0, 0.0];
    let mut losses = LakeLosses {
        evaporation_demand_m3: vec![[0.0; 4]; g.len()],
        groundwater_export_demand_m3: vec![[0.0; 4]; g.len()],
    };
    losses.evaporation_demand_m3[15] = [0.0, 100.0, 0.0, 0.0];
    let p = solve_with_lake_losses(&g, &dem, &vec![false; g.len()], &input, &losses).unwrap();
    profile("H05_lake_pet", &g, &dem, &p);
    assert_eq!(p.seasonal_storage_m3(), [20.0, 7.0, 7.0, 7.0]);
    assert_eq!(p.lake_evaporation_m3[15][1], 13.0);
    assert_eq!(p.seasonal_water_level_m[15][1], None);
    assert_eq!(p.seasonal_water_level_m[19][1], Some(4.0));
    let owned: usize = p.depressions.iter().map(|d| d.cells.len()).sum();
    assert!(owned <= g.len());
}

#[test]
fn marine_precipitation_exits_without_terrestrial_reservoirs() {
    let g = Grid {
        columns: 3,
        rows: 1,
        min_x_m: 0.0,
        min_z_m: 0.0,
        width_m: 5.0,
        depth_m: 1.0,
        step_m: 2.0,
    };
    let ocean = [true; 3];
    let precipitation = [
        [100.0, 200.0, 300.0, 400.0],
        [50.0, 0.0, 200.0, 0.0],
        [0.0, 150.0, 0.0, 250.0],
    ];
    let temperature = [[-20.0, 5.0, 25.0, -10.0]; 3];
    let pet = [[5000.0; 4]; 3];
    let snow_fraction = [[1.0, 0.25, 0.0, 1.0]; 3];
    let p = runoff(
        &g,
        &temperature,
        &precipitation,
        &pet,
        &snow_fraction,
        &ocean,
    )
    .unwrap();
    for (i, rain) in precipitation.iter().enumerate() {
        assert_eq!(p.precipitation_m3[i], rain.map(|v| v * (g.area(i) * 0.001)));
        assert_eq!(p.local_m3[i], p.precipitation_m3[i]);
        for field in [
            &p.actual_et_m3,
            &p.snowfall_m3,
            &p.snowmelt_m3,
            &p.infiltration_m3,
            &p.recharge_m3,
            &p.baseflow_m3,
            &p.snow_storage_m3,
            &p.soil_storage_m3,
            &p.groundwater_storage_m3,
        ] {
            assert_eq!(field[i], [0.0; 4]);
        }
    }
    let hydro = fixture(
        "H07_marine",
        &g,
        &[-2000.0, -75.0, -100.0],
        &ocean,
        &p.local_m3,
    );
    assert_eq!(hydro.external_outflow_m3, [0.0; 4]);
    assert_eq!(hydro.seasonal_storage_m3(), [0.0; 4]);
    for (s, expected) in [0.3, 0.55, 1.0, 1.05].into_iter().enumerate() {
        let precipitation: f64 = p.precipitation_m3.iter().map(|v| v[s]).sum();
        assert!(close(hydro.ocean_outflow_m3[s], expected));
        assert!(close(precipitation, hydro.ocean_outflow_m3[s]));
    }
    assert!(
        runoff(
            &g,
            &temperature,
            &precipitation,
            &pet,
            &snow_fraction,
            &[true; 2]
        )
        .is_err()
    );
    assert!(p.validate(&g, &[true; 2]).is_err());
    let mut corrupt = p.clone();
    corrupt.actual_et_m3[0][0] = 0.1;
    corrupt.local_m3[0][0] -= 0.1;
    assert!(corrupt.validate(&g, &ocean).is_err());
    let mut corrupt = p;
    corrupt.soil_storage_m3[0][0] = 0.1;
    corrupt.local_m3[0][0] -= 0.1;
    assert!(corrupt.validate(&g, &ocean).is_err());
}

#[test]
fn large_physical_cells_preserve_runoff_balance_without_hiding_water_creation() {
    let temperature = [[20.0; 4]];
    let precipitation = [[
        844.4218515250482,
        757.9544029403024,
        420.571580830845,
        258.91675029296334,
    ]];
    let pet = [[
        153.38241641058255,
        121.48024123512428,
        235.13957671043178,
        90.99381782367824,
    ]];
    let small = grid(1, 1);
    let large = Grid {
        step_m: 250_000.0,
        width_m: 250_000.0,
        depth_m: 250_000.0,
        ..grid(1, 1)
    };
    let small_plan = runoff(
        &small,
        &temperature,
        &precipitation,
        &pet,
        &[[0.0; 4]],
        &[false],
    )
    .unwrap();
    let large_plan = runoff(
        &large,
        &temperature,
        &precipitation,
        &pet,
        &[[0.0; 4]],
        &[false],
    )
    .unwrap();
    for season in 0..4 {
        assert!(close(
            small_plan.local_m3[0][season],
            large_plan.local_m3[0][season] / large.area(0),
        ));
        assert!(close(
            small_plan.actual_et_m3[0][season],
            large_plan.actual_et_m3[0][season] / large.area(0),
        ));
    }
    let exhausted = runoff(
        &large,
        &temperature,
        &[[
            14.030927323372037,
            35.42301210811698,
            32.91323856929842,
            17.65207077218265,
        ]],
        &[[
            4.954350870919409,
            4.494910647887381,
            6.515929727227629,
            200.0,
        ]],
        &[[0.0; 4]],
        &[false],
    )
    .unwrap();
    assert_eq!(exhausted.local_m3[0][3], 0.0);
    assert_eq!(exhausted.soil_storage_m3[0][3], 0.0);
    let mut corrupted = large_plan;
    corrupted.actual_et_m3[0][2] += large.area(0) * 0.001;
    assert!(corrupted.validate(&large, &[false]).is_err());
}
