use super::super::definition::{DefinitionInput, PlaneBounds};
use super::super::prepared::{PrepareOptions, PreparedWorld, inspect_committed, prepare};
use super::*;

fn small(seed: i64) -> WorldDefinition {
    let mut d = DefinitionInput::earth_root(seed);
    d.domain = PlaneBounds {
        min_x: -1024,
        max_x_exclusive: 1024,
        min_z: -1024,
        max_z_exclusive: 1024,
    };
    d.macro_cell_m = 256.0;
    d.climate_cell_m = 512.0;
    d.validate().unwrap()
}

#[test]
fn physical_root_cohort_has_complete_traces_and_stable_roundtrip() {
    for seed in [
        0,
        1,
        -1,
        42,
        712816,
        i64::MIN,
        i64::MAX,
        7,
        19,
        101,
        1234,
        -99,
        2026,
        4096,
        65537,
        -712816,
    ] {
        let d = small(seed);
        let root = WorldPlan::build(&d).unwrap();
        let bytes = root.canonical_bytes().unwrap();
        let decoded = WorldPlan::from_canonical_bytes(&bytes, &d).unwrap();
        assert_eq!(decoded, root, "seed {seed}");
        assert_eq!(decoded.canonical_bytes().unwrap(), bytes, "seed {seed}");
        let p = root.plane().unwrap();
        for start in 0..p.grid.len() {
            let mut cell = start;
            let mut steps = 0;
            while let Some(next) = p.hydrology.downstream[cell] {
                assert!(p.hydrology.rank[next] < p.hydrology.rank[cell]);
                steps += 1;
                assert!(steps < p.grid.len());
                cell = next;
            }
        }
        if let Ok(path) = std::env::var("SOLARIS_W2_ARTIFACT_DIR") {
            std::fs::create_dir_all(&path).unwrap();
            let mut csv = String::from(
                "cell,x_m,z_m,area_m2,elevation_m,ocean,downstream,annual_discharge_m3,annual_precip_mm\n",
            );
            for i in 0..p.grid.len() {
                let (x, z) = p.grid.center(i);
                let ci = p.climate_grid.index_at(x, z);
                csv.push_str(&format!(
                    "{i},{x},{z},{},{},{},{},{},{}\n",
                    p.grid.area(i),
                    p.elevation_m[i],
                    p.ocean[i],
                    p.hydrology.downstream[i].map_or_else(|| "terminal".into(), |j| j.to_string()),
                    p.hydrology.discharge_m3[i].iter().sum::<f64>(),
                    p.climate.cells[ci].precipitation_mm.iter().sum::<f64>()
                ));
            }
            std::fs::write(
                std::path::Path::new(&path).join(format!("root-seed-{seed}.csv")),
                csv,
            )
            .unwrap();
        }
    }
}

#[test]
fn plane_publication_budgets_and_operating_bounds_preserve_geography() {
    let d = small(42);
    let a_dir = tempfile::tempdir().unwrap();
    let b_dir = tempfile::tempdir().unwrap();
    let subset = PlaneBounds {
        min_x: 0,
        max_x_exclusive: 16,
        min_z: 0,
        max_z_exclusive: 16,
    };
    let mut a = prepare(
        a_dir.path(),
        &d,
        subset,
        PrepareOptions {
            native_cache_bytes: 2 * 1024 * 1024,
            ..PrepareOptions::default()
        },
    )
    .unwrap();
    let b = prepare(
        b_dir.path(),
        &d,
        d.input().domain,
        PrepareOptions {
            native_cache_bytes: 16 * 1024 * 1024,
            build_scratch_limit_bytes: 512 * 1024 * 1024,
        },
    )
    .unwrap();
    let original = a.root_plan().canonical_bytes().unwrap();
    assert_eq!(a.identity(), b.identity());
    assert_eq!(original, b.root_plan().canonical_bytes().unwrap());
    a.set_bounds(d.input().domain).unwrap();
    a.set_bounds(subset).unwrap();
    assert_eq!(original, a.root_plan().canonical_bytes().unwrap());
    let identity = a.identity();
    drop(a);
    let report = inspect_committed(a_dir.path(), Some(&d), PrepareOptions::default()).unwrap();
    assert_eq!(identity, report.identity);
    assert_eq!(original, report.root_plan.canonical_bytes().unwrap());
    drop(report);
    let mut changed = d.input().clone();
    changed.seed = -1;
    assert!(matches!(
        PreparedWorld::open(
            a_dir.path(),
            Some(&changed.validate().unwrap()),
            PrepareOptions::default()
        ),
        Err(PrepareError::IdentityMismatch)
    ));
}

#[test]
fn partial_cells_conserve_area_across_noncommensurate_climate_grid() {
    let mut input = DefinitionInput::earth_root(42);
    input.domain = PlaneBounds {
        min_x: -96,
        max_x_exclusive: 80,
        min_z: -64,
        max_z_exclusive: 80,
    };
    input.macro_cell_m = 48.0;
    input.climate_cell_m = 70.0;
    let d = input.validate().unwrap();
    let grid = Grid::from_definition(&d, 48.0).unwrap();
    let climate = Grid::from_definition(&d, 70.0).unwrap();
    let mut totals = vec![0.0; climate.len()];
    for i in 0..grid.len() {
        let pieces: Vec<_> = overlaps(&grid, i, &climate).collect();
        assert!((pieces.iter().map(|(_, a)| a).sum::<f64>() - grid.area(i)).abs() < 1e-8);
        for (j, a) in pieces {
            totals[j] += a;
        }
    }
    for (i, a) in totals.into_iter().enumerate() {
        assert!((a - climate.area(i)).abs() < 1e-8);
    }
    assert!(
        ((0..grid.len()).map(|i| grid.area(i)).sum::<f64>() - d.estimate().unwrap().area_m2).abs()
            < 1e-8
    );
}

#[test]
fn root_rejects_foreign_geometry_and_payload_corruption() {
    let d = small(1);
    let root = WorldPlan::build(&d).unwrap();
    let mut bytes = root.canonical_bytes().unwrap();
    let mut foreign = d.input().clone();
    foreign.domain.max_x_exclusive += 256;
    assert!(WorldPlan::from_canonical_bytes(&bytes, &foreign.validate().unwrap()).is_err());
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    assert!(WorldPlan::from_canonical_bytes(&bytes, &d).is_err());
    let dir = tempfile::tempdir().unwrap();
    assert!(
        prepare(
            dir.path(),
            &d,
            d.input().domain,
            PrepareOptions {
                build_scratch_limit_bytes: 1,
                ..PrepareOptions::default()
            }
        )
        .is_err()
    );
    assert!(!dir.path().join("world.manifest.toml").exists());
}

#[test]
fn marine_forcing_and_full_water_balance_use_surface_not_bottom() {
    let d = small(42);
    let g = Grid::from_definition(&d, d.input().macro_cell_m).unwrap();
    let c = Grid::from_definition(&d, d.input().climate_cell_m).unwrap();
    let ocean = vec![true; g.len()];
    let bottom: Vec<_> = (0..g.len())
        .map(|i| -6000.0 + (i % g.columns) as f64 * 600.0)
        .collect();
    let (climate, forcing, runoff, hydro) = physical_water(&d, &g, &c, &bottom, &ocean).unwrap();
    for i in 0..g.len() {
        let (x, z) = g.center(i);
        let j = c.index_at(x, z);
        assert_eq!(forcing.temperature_c[i], climate.cells[j].temperature_c);
        assert_eq!(runoff.actual_et_m3[i], [0.0; 4]);
        assert_eq!(runoff.soil_storage_m3[i], [0.0; 4]);
        assert_eq!(runoff.groundwater_storage_m3[i], [0.0; 4]);
    }
    for s in 0..4 {
        let rain: f64 = runoff.precipitation_m3.iter().map(|p| p[s]).sum();
        assert!((rain - hydro.ocean_outflow_m3[s]).abs() <= rain.max(1.0) * 1e-12);
        assert_eq!(hydro.external_outflow_m3[s], 0.0);
    }
}

#[test]
fn basin_ownership_follows_complete_drainage_and_canonical_lake_spill() {
    let root = WorldPlan::build(&small(42)).unwrap();
    let mut hydro = root.plane().unwrap().hydrology.clone();
    let n = hydro.rank.len();
    let split = n / 2;
    hydro.rank = (0..n).collect();
    hydro.downstream = (0..n)
        .map(|i| {
            if i == 0 || i == split {
                None
            } else {
                Some(i - 1)
            }
        })
        .collect();
    hydro.depression.fill(None);
    hydro.depressions.clear();
    let terminal_owners = basin_owners(&hydro);
    assert!(terminal_owners[..split].iter().all(|&i| i == 0));
    assert!(terminal_owners[split..].iter().all(|&i| i == split));
    hydro.depressions.push(hydrology::Depression {
        id: 0,
        parent: None,
        children: Vec::new(),
        cells: (15..21).collect(),
        minimum_m: 0.0,
        spill_level_m: 1.0,
        spill_from: 15,
        spill_to: 14,
        capacity_m3: 1.0,
        storage_m3: [1.0; 4],
        level_m: [Some(1.0); 4],
        inflow_m3: [0.0; 4],
        outflow_m3: [0.0; 4],
        evaporation_m3: [0.0; 4],
        groundwater_export_m3: [0.0; 4],
    });
    hydro.depression[15..21].fill(Some(0));
    let lake_owners = basin_owners(&hydro);
    assert!(lake_owners[..15].iter().all(|&i| i == 0));
    assert!(lake_owners[15..split].iter().all(|&i| i == 15));
    assert!(lake_owners[split..].iter().all(|&i| i == split));
}

#[test]
fn canonical_root_rejects_changed_basin_ownership() {
    let d = small(42);
    let mut root = WorldPlan::build(&d).unwrap();
    let WorldPlan::Plane(p) = &mut root else {
        panic!("plane fixture");
    };
    p.basin_owner[0] = (p.basin_owner[0] + 1) % p.grid.len();
    let bytes = root.canonical_bytes().unwrap();
    assert!(matches!(
        WorldPlan::from_canonical_bytes(&bytes, &d),
        Err(PrepareError::Invalid("root basin ownership mismatch"))
    ));
}

#[test]
fn canonical_root_rejects_changed_water_body_ownership() {
    let d = small(42);
    let mut root = WorldPlan::build(&d).unwrap();
    let WorldPlan::Plane(p) = &mut root else {
        panic!("plane fixture");
    };
    p.water_body_owner[0][0] = match p.water_body_owner[0][0] {
        None => Some(0),
        Some(_) => None,
    };
    let bytes = root.canonical_bytes().unwrap();
    assert!(matches!(
        WorldPlan::from_canonical_bytes(&bytes, &d),
        Err(PrepareError::Invalid("root water body ownership mismatch"))
    ));
}
