use super::super::definition::{DefinitionInput, PlaneBounds};
use super::super::tectonics::{BoundaryRegime, BoundarySegment, Footprint, Province, Structure};
use super::super::world_plan::WorldPlan;
use super::*;

fn definition() -> super::super::definition::WorldDefinition {
    let mut d = DefinitionInput::earth_root(42);
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

fn fixture() -> PlanePlan {
    let root = WorldPlan::build(&definition()).unwrap();
    let mut p = root.plane().unwrap().clone();
    p.elevation_m.fill(250.0);
    p.ocean.fill(false);
    p.tectonics.elevation_m.fill(300.0);
    p.tectonics.erodibility.fill(0.2);
    p.tectonics.cold_phase.fill(false);
    for c in &mut p.tectonics.crust {
        c.continental_fraction = 1.0;
        c.thickness_m = 30_000.0;
        c.basement_age_ma = 1000.0;
        c.lithology_id = 1;
        c.sediment_cover_m = 200.0;
    }
    p.tectonics.boundaries.clear();
    p.tectonics.structures.clear();
    p.tectonics.provinces = vec![Province {
        id: 73,
        kind: ProvinceKind::AncientOrogen,
        footprint: Footprint {
            center: Point { x_m: 0.0, z_m: 0.0 },
            radius_x_m: 100_000.0,
            radius_z_m: 100_000.0,
            angle_rad: 0.0,
        },
        age_ma: 300.0,
        initial_relief_m: 1000.0,
        basement_lithology_id: 2,
        sediment_cover_m: 200.0,
        folding: 0.9,
        intrusion_fraction: 0.2,
        mineralization: 0.1,
    }];
    p.climate_forcing.temperature_c.fill([15.0; 4]);
    p.climate_forcing.precipitation_mm.fill([300.0; 4]);
    p.climate_forcing.potential_et_mm.fill([100.0; 4]);
    p.hydrology.seasonal_water_level_m.fill([None; 4]);
    for i in 0..p.grid.len() {
        p.runoff.soil_storage_m3[i] = [p.grid.area(i) * 0.03; 4];
        p.runoff.snow_storage_m3[i] = [0.0; 4];
        p.runoff.snowmelt_m3[i] = [0.0; 4];
        p.runoff.baseflow_m3[i] = [0.0; 4];
    }
    for pass in &mut p.erosion_passes {
        pass.source_m3.fill([0.0; 2]);
        pass.deposited_m3.fill([0.0; 2]);
    }
    p.region_geology = GeologyContext::build(&p.grid, &p.tectonics).unwrap();
    p
}

fn ordered(c: &GeologyColumn) {
    for b in &c.bands {
        assert!(b.bottom_m.is_finite() && b.top_m.is_finite());
        assert!(b.bottom_m <= b.top_m);
        assert!(b.permeability_m_s > 0.0);
    }
    for pair in c.bands.windows(2) {
        assert_eq!(pair[0].top_m, pair[1].bottom_m);
        assert!(pair[0].age_ma >= pair[1].age_ma);
    }
}

#[test]
fn folded_interfaces_are_ordered_continuous_and_inherit_history() {
    let p = fixture();
    let a = sample(&p, -128.0 - 1e-5, -128.0, 250.0, false);
    let b = sample(&p, -128.0 + 1e-5, -128.0, 250.0, false);
    ordered(&a.geology);
    ordered(&b.geology);
    for (left, right) in a.geology.bands.iter().zip(&b.geology.bands) {
        assert!((left.bottom_m - right.bottom_m).abs() < 1e-5);
        assert!((left.top_m - right.top_m).abs() < 1e-5);
    }
    assert_eq!(a.geology.bands[0].lithology, RockClass::Metamorphic);
    assert!((a.geology.bands[1].age_ma - 150.0).abs() < 1e-10);
    assert!((a.geology.provenance.iter().map(|v| v.weight).sum::<f64>() - 1.0).abs() < 1e-12);
    for source in a.geology.provenance.iter().filter(|v| v.weight > 0.0) {
        assert_eq!(source.province_id, Some(73));
        assert_eq!(source.plate_id, p.tectonics.plate_id[source.root_cell]);
    }
    let north = sample(&p, 0.0, -384.0, 250.0, false);
    let south = sample(&p, 0.0, 384.0, 250.0, false);
    assert!(south.geology.bands[0].top_m - north.geology.bands[0].top_m > 5.0);
    let west = sample(&p, -384.0, 0.0, 250.0, false);
    let east = sample(&p, 384.0, 0.0, 250.0, false);
    assert!((west.geology.bands[0].top_m - east.geology.bands[0].top_m).abs() < 1e-10);
    // Cutting below the geological datum exposes basement rather than moving all strata down.
    let exposed = sample(&p, 0.0, 0.0, 50.0, false);
    ordered(&exposed.geology);
    assert_eq!(
        exposed.geology.bands[1].bottom_m,
        exposed.geology.bands[1].top_m
    );
    assert_eq!(
        exposed.geology.bands[2].bottom_m,
        exposed.geology.bands[2].top_m
    );
    assert_eq!(exposed.ecology.parent_material, RockClass::Metamorphic);
}

#[test]
fn fault_throw_displaces_and_exposes_cover_with_boundary_provenance() {
    let mut p = fixture();
    p.tectonics.provinces[0].folding = 0.0;
    p.tectonics.boundaries.push(BoundarySegment {
        id: 91,
        plate_a: 0,
        plate_b: 1,
        points: vec![
            Point {
                x_m: -1024.0,
                z_m: 0.0,
            },
            Point {
                x_m: 1024.0,
                z_m: 0.0,
            },
        ],
        relative_velocity_m_per_ma: Point {
            x_m: 1000.0,
            z_m: 0.0,
        },
        normal_motion_m_per_ma: 0.0,
        shear_m_per_ma: 1000.0,
        regime: BoundaryRegime::Transform,
        subducting_plate: None,
        backarc_extension: false,
        duration_ma: 1.0,
    });
    p.tectonics.structures.push(Structure {
        boundary_id: 91,
        basement_lithology_id: 1,
        folding: 0.0,
        fault_displacement_m: 1000.0,
        intrusion_fraction: 0.0,
        mineralization: 0.0,
        trench_offset_m: 0.0,
        forearc_offset_m: 0.0,
        arc_offset_m: 0.0,
        backarc_offset_m: 0.0,
    });
    p.region_geology = GeologyContext::build(&p.grid, &p.tectonics).unwrap();
    let a = sample(&p, 0.0, -384.0, 250.0, false);
    let b = sample(&p, 0.0, 384.0, 250.0, false);
    ordered(&a.geology);
    ordered(&b.geology);
    assert!(a.geology.fault_displacement_m < -800.0);
    assert!(b.geology.fault_displacement_m > 800.0);
    assert!(a.geology.bands[1].top_m > a.geology.bands[1].bottom_m);
    assert_eq!(b.geology.bands[1].top_m, b.geology.bands[1].bottom_m);
    assert!(
        b.geology
            .provenance
            .iter()
            .all(|v| v.boundary_id == Some(91))
    );
}

#[test]
fn deposit_texture_controls_drainage_and_climate_controls_soil_and_deficit() {
    let mut fine = fixture();
    fine.climate_forcing.precipitation_mm.fill([30.0; 4]);
    for i in 0..fine.grid.len() {
        fine.erosion_passes[0].deposited_m3[i] = [0.0, fine.grid.area(i)];
    }
    let mut coarse = fine.clone();
    for i in 0..coarse.grid.len() {
        coarse.erosion_passes[0].deposited_m3[i] = [coarse.grid.area(i), 0.0];
    }
    let f = sample(&fine, 0.0, 0.0, 250.0, false);
    let c = sample(&coarse, 0.0, 0.0, 250.0, false);
    assert_eq!(f.ecology.parent_material, RockClass::FineSediment);
    assert_eq!(c.ecology.parent_material, RockClass::CoarseSediment);
    assert!(f.ecology.moisture_fraction[0] > c.ecology.moisture_fraction[0] * 5.0);
    assert!((f.geology.bands[3].top_m - f.geology.bands[3].bottom_m - 1.0).abs() < 1e-12);
    assert_eq!(f.ecology.water_deficit_mm, [70.0; 4]);
    let mut rainy = fine.clone();
    rainy.climate_forcing.precipitation_mm.fill([300.0; 4]);
    let r = sample(&rainy, 0.0, 0.0, 250.0, false);
    assert!(r.ecology.soil_depth_m > f.ecology.soil_depth_m * 3.0);
    assert_eq!(r.ecology.water_deficit_mm, [0.0; 4]);
    let mut cold = rainy.clone();
    cold.climate_forcing.temperature_c.fill([-15.0; 4]);
    let snow = sample(&cold, 0.0, 0.0, 250.0, false);
    assert_eq!(snow.ecology.soil_depth_m, 0.0);
    assert_eq!(snow.ecology.snow_fraction, [1.0; 4]);
    assert_eq!(snow.ecology.water_deficit_mm, [100.0; 4]);
}

#[test]
fn ecology_has_physical_lapse_slope_and_seasonal_water_context() {
    let mut p = fixture();
    let low = sample(&p, 0.0, 0.0, 250.0, false);
    let high = sample(&p, 0.0, 0.0, 1250.0, false);
    assert!((low.ecology.temperature_c[0] - high.ecology.temperature_c[0] - 6.0).abs() < 1e-12);
    for i in 0..p.grid.len() {
        p.elevation_m[i] = 250.0 + p.grid.center(i).0;
    }
    let steep = sample(&p, 0.0, 0.0, 250.0, false);
    assert!((steep.ecology.slope - 1.0).abs() < 1e-12);
    assert!(steep.ecology.soil_depth_m < low.ecology.soil_depth_m * 0.2);
    assert!(steep.ecology.rock_exposure > low.ecology.rock_exposure);
    p.elevation_m.fill(250.0);
    p.hydrology
        .seasonal_water_level_m
        .fill([Some(252.0), None, Some(252.0), None]);
    let flooded = sample(&p, 0.0, 0.0, 250.0, true);
    assert_eq!(flooded.ecology.hydroperiod_fraction, 0.5);
    assert!(flooded.ecology.soil_depth_m < low.ecology.soil_depth_m);
    for i in 0..p.grid.len() {
        p.ocean[i] = p.grid.center(i).0 < 0.0;
        p.runoff.snow_storage_m3[i] = [p.grid.area(i) * 0.05, 0.0, 0.0, 0.0];
    }
    let shore = sample(&p, 0.0, 0.0, 0.0, true);
    let inland = sample(&p, 512.0, 0.0, 250.0, false);
    assert_eq!(shore.ecology.ocean_fraction, 0.5);
    assert_eq!(shore.ecology.shore_fraction, 1.0);
    assert_eq!(inland.ecology.shore_fraction, 0.0);
    assert_eq!(shore.ecology.snow_water_mm[0], 50.0);
}

#[test]
fn root_decode_rejects_geological_history_not_derived_from_tectonics() {
    let d = definition();
    let mut root = WorldPlan::build(&d).unwrap();
    let WorldPlan::Plane(p) = &mut root else {
        panic!("plane fixture");
    };
    p.region_geology.cells[0].sediment_age_ma += 1.0;
    let bytes = root.canonical_bytes().unwrap();
    assert!(matches!(
        WorldPlan::from_canonical_bytes(&bytes, &d),
        Err(PrepareError::Invalid("root geology provenance mismatch"))
    ));
}

#[test]
fn inherited_marine_cover_changes_solubility_and_exposed_substrate_drainage() {
    let continental = fixture();
    let mut marine = continental.clone();
    for c in &mut marine.tectonics.crust {
        c.continental_fraction = 0.0;
    }
    marine.region_geology = GeologyContext::build(&marine.grid, &marine.tectonics).unwrap();
    // Erosion exposes the lower sedimentary unit in both environments.
    let land = sample(&continental, 0.0, 0.0, 200.0, false);
    let sea = sample(&marine, 0.0, 0.0, 200.0, false);
    assert_eq!(land.ecology.parent_material, RockClass::Sandstone);
    assert_eq!(sea.ecology.parent_material, RockClass::Carbonate);
    assert!(sea.geology.bands[1].solubility > land.geology.bands[1].solubility * 10.0);
    assert!(sea.ecology.moisture_fraction[0] > land.ecology.moisture_fraction[0]);
    ordered(&sea.geology);
}

#[test]
fn partial_outer_cell_uses_physical_centres_for_gradient_and_temperature() {
    let mut p = fixture();
    p.grid.width_m = 1930.0;
    p.grid.validate().unwrap();
    for i in 0..p.grid.len() {
        p.elevation_m[i] = 250.0 + p.grid.center(i).0;
    }
    let local = sample(&p, 800.0, 0.0, 1050.0, false);
    assert!((local.ecology.slope - 1.0).abs() < 1e-12);
    assert!((local.ecology.temperature_c[0] - 15.0).abs() < 1e-12);
    ordered(&local.geology);
}

#[test]
fn changing_dem_gradients_do_not_split_soil_interfaces_at_root_knots() {
    let mut p = fixture();
    for i in 0..p.grid.len() {
        let x = p.grid.center(i).0;
        p.elevation_m[i] = 250.0 + x * x / 1000.0;
    }
    let ground = 250.0 + 128.0 * 128.0 / 1000.0;
    let a = sample(&p, -128.0 - 1e-6, 0.0, ground, false);
    let b = sample(&p, -128.0 + 1e-6, 0.0, ground, false);
    assert!((a.ecology.slope - b.ecology.slope).abs() < 1e-7);
    assert!((a.geology.bands[4].bottom_m - b.geology.bands[4].bottom_m).abs() < 1e-5);
}
