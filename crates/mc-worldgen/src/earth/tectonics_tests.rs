use super::*;

struct Fixture {
    grid: Grid,
    plates: Vec<Plate>,
    ids: Vec<usize>,
    crust: Vec<CrustCell>,
    boundaries: Vec<BoundarySegment>,
    provinces: Vec<Province>,
    hotspots: Vec<Hotspot>,
}
impl Fixture {
    fn pair(continental_a: bool, continental_b: bool, relative: Point) -> Self {
        let grid = Grid {
            columns: 161,
            rows: 9,
            min_x_m: -402_500.0,
            min_z_m: -22_500.0,
            width_m: 805_000.0,
            depth_m: 45_000.0,
            step_m: 5_000.0,
        };
        let plates = vec![
            Plate {
                id: 0,
                nucleus: Point {
                    x_m: -200_000.0,
                    z_m: 0.0,
                },
                growth_weight: 1.0,
                velocity_history: vec![VelocityEpoch {
                    start_age_ma: 0.0,
                    velocity_m_per_ma: relative.scale(-0.5),
                }],
            },
            Plate {
                id: 1,
                nucleus: Point {
                    x_m: 200_000.0,
                    z_m: 0.0,
                },
                growth_weight: 1.0,
                velocity_history: vec![VelocityEpoch {
                    start_age_ma: 0.0,
                    velocity_m_per_ma: relative.scale(0.5),
                }],
            },
        ];
        let ids = (0..grid.len())
            .map(|i| usize::from(point(&grid, i).x_m > 0.0))
            .collect();
        let crust = (0..grid.len())
            .map(|i| {
                let a = point(&grid, i).x_m <= 0.0;
                let continental = if a { continental_a } else { continental_b };
                CrustCell {
                    continental_fraction: if continental { 1.0 } else { 0.0 },
                    thickness_m: if continental { 35_000.0 } else { 7_000.0 },
                    basement_age_ma: if continental {
                        1500.0
                    } else if a {
                        120.0
                    } else {
                        60.0
                    },
                    ocean_floor_age_ma: if a { 120.0 } else { 60.0 },
                    lithology_id: if continental { 1 } else { 0 },
                    sediment_cover_m: 0.0,
                }
            })
            .collect::<Vec<_>>();
        let points = vec![
            Point {
                x_m: 0.0,
                z_m: -300_000.0,
            },
            Point {
                x_m: 0.0,
                z_m: 300_000.0,
            },
        ];
        let boundaries = vec![classify_segment(0, 0, 1, points, &plates, &grid, &crust)];
        Self {
            grid,
            plates,
            ids,
            crust,
            boundaries,
            provinces: Vec::new(),
            hotspots: Vec::new(),
        }
    }
    fn run(self) -> TectonicPlan {
        assemble(
            &self.grid,
            self.plates,
            self.ids,
            Vec::new(),
            self.crust,
            self.boundaries,
            self.provinces,
            self.hotspots,
        )
        .unwrap()
    }
    fn single(continental: bool) -> Self {
        let mut f = Self::pair(continental, continental, Point { x_m: 0.0, z_m: 0.0 });
        f.plates.truncate(1);
        f.ids.fill(0);
        f.boundaries.clear();
        f
    }
}
fn sample(plan: &TectonicPlan, grid: &Grid, x: f64, z: f64) -> f64 {
    plan.elevation_m[grid.index_at(x, z)]
}
fn ancient(age_ma: f64) -> Province {
    Province {
        id: 0,
        kind: ProvinceKind::AncientOrogen,
        footprint: Footprint {
            center: Point { x_m: 0.0, z_m: 0.0 },
            radius_x_m: 80_000.0,
            radius_z_m: 300_000.0,
            angle_rad: 0.0,
        },
        age_ma,
        initial_relief_m: 3000.0,
        basement_lithology_id: 2,
        sediment_cover_m: 100.0,
        folding: 0.9,
        intrusion_fraction: 0.3,
        mineralization: 0.2,
    }
}

/// Fixture evidence is opt-in and deterministic. The CSV contains the complete
/// physical grid; SVG contains both a height map and a signed cross-section.
fn evidence(name: &str, grid: &Grid, plan: &TectonicPlan) {
    let Some(directory) = std::env::var_os("SOLARIS_W2_ARTIFACT_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    use std::fmt::Write as _;
    let mut csv = String::from(
        "x_m,z_m,elevation_m,erodibility,continental_fraction,thickness_m,ocean_floor_age_ma,plate_id,lithology_id,sediment_cover_m,cold_phase\n",
    );
    for i in 0..grid.len() {
        let p = point(grid, i);
        let c = &plan.crust[i];
        writeln!(
            csv,
            "{},{},{},{},{},{},{},{},{},{},{}",
            p.x_m,
            p.z_m,
            plan.elevation_m[i],
            plan.erodibility[i],
            c.continental_fraction,
            c.thickness_m,
            c.ocean_floor_age_ma,
            plan.plate_id[i],
            c.lithology_id,
            c.sediment_cover_m,
            plan.cold_phase[i]
        )
        .unwrap();
    }
    std::fs::write(directory.join(format!("{name}.csv")), csv).unwrap();
    let min = plan
        .elevation_m
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min)
        .min(0.0);
    let max = plan
        .elevation_m
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max)
        .max(1.0);
    let span = (max - min).max(1.0);
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1000\" height=\"650\" viewBox=\"0 0 1000 650\"><rect width=\"1000\" height=\"650\" fill=\"white\"/><text x=\"20\" y=\"22\">{name}: physical height map and z=0 cross-section (metres)</text>"
    );
    for i in 0..grid.len() {
        let v = ((plan.elevation_m[i] - min) / span * 255.0).round() as u8;
        let x = 40.0 + (i % grid.columns) as f64 / grid.columns as f64 * 920.0;
        let z = 35.0 + (i / grid.columns) as f64 / grid.rows as f64 * 285.0;
        write!(
            svg,
            "<rect x=\"{x}\" y=\"{z}\" width=\"{}\" height=\"{}\" fill=\"rgb({v},{},{})\"/>",
            920.0 / grid.columns as f64 + 0.1,
            285.0 / grid.rows as f64 + 0.1,
            255 - v,
            v / 2
        )
        .unwrap();
    }
    for v in &plan.volcanoes {
        if v.center.x_m < grid.min_x_m
            || v.center.x_m >= grid.min_x_m + grid.width_m
            || v.center.z_m < grid.min_z_m
            || v.center.z_m >= grid.min_z_m + grid.depth_m
        {
            continue;
        }
        let x = 40.0 + (v.center.x_m - grid.min_x_m) / grid.width_m * 920.0;
        let z = 35.0 + (v.center.z_m - grid.min_z_m) / grid.depth_m * 285.0;
        write!(svg, "<circle cx=\"{x}\" cy=\"{z}\" r=\"4\" fill=\"white\" stroke=\"black\"/><text x=\"{}\" y=\"{}\" font-size=\"12\">{} Ma</text>",
            x+6.0,z-5.0,v.age_ma).unwrap();
    }
    let y_zero = 610.0 - (0.0 - min) / span * 250.0;
    write!(svg,"<path d=\"M40 {y_zero} H960\" stroke=\"#999\"/><text x=\"5\" y=\"{y_zero}\">0m</text><polyline fill=\"none\" stroke=\"#123456\" stroke-width=\"2\" points=\"").unwrap();
    for x in 0..grid.columns {
        let i = grid.index_at(grid.min_x_m + (x as f64 + 0.5) * grid.step_m, 0.0);
        let sx = 40.0 + x as f64 / (grid.columns - 1) as f64 * 920.0;
        let sy = 610.0 - (plan.elevation_m[i] - min) / span * 250.0;
        write!(svg, "{sx},{sy} ").unwrap();
    }
    write!(svg,"\"/><text x=\"40\" y=\"640\">x={} .. {} m; height={} .. {} m; oceanic is crust, not water</text></svg>",grid.min_x_m,grid.min_x_m+grid.width_m,min,max).unwrap();
    std::fs::write(directory.join(format!("{name}.svg")), svg).unwrap();
    if !plan.volcanoes.is_empty() {
        let mut chain = String::from("hotspot_id,plate_id,x_m,z_m,age_ma,height_m,subsidence_m\n");
        for v in &plan.volcanoes {
            writeln!(
                chain,
                "{},{},{},{},{},{},{}",
                v.hotspot_id,
                v.plate_id,
                v.center.x_m,
                v.center.z_m,
                v.age_ma,
                v.height_m,
                v.subsidence_m
            )
            .unwrap();
        }
        std::fs::write(directory.join(format!("{name}_chain.csv")), chain).unwrap();
    }
}

#[test]
fn t01_continental_collision_thick_relief_without_automatic_arc() {
    let f = Fixture::pair(
        true,
        true,
        Point {
            x_m: -40_000.0,
            z_m: 0.0,
        },
    );
    let g = f.grid.clone();
    let p = f.run();
    assert_eq!(p.boundaries[0].regime, BoundaryRegime::Collision);
    assert!(sample(&p, &g, 0.0, 0.0) > sample(&p, &g, 200_000.0, 0.0) + 1000.0);
    assert!(sample(&p, &g, 80_000.0, 0.0) > sample(&p, &g, 350_000.0, 0.0) + 1500.0);
    assert!(p.structures[0].folding > 0.8);
    assert!(
        p.crust[g.index_at(0.0, 0.0)].thickness_m
            > p.crust[g.index_at(350_000.0, 0.0)].thickness_m + 10_000.0
    );
    assert!(p.structures[0].intrusion_fraction < 0.1);
    assert_eq!(p.boundaries[0].subducting_plate, None);
    evidence("T01_collision", &g, &p);
}

#[test]
fn t02_ocean_continent_trench_forearc_arc_correct_sides() {
    let f = Fixture::pair(
        false,
        true,
        Point {
            x_m: -40_000.0,
            z_m: 0.0,
        },
    );
    let g = f.grid.clone();
    let p = f.run();
    assert_eq!(p.boundaries[0].regime, BoundaryRegime::Subduction);
    assert_eq!(p.boundaries[0].subducting_plate, Some(0));
    let trench = sample(&p, &g, -35_000.0, 0.0);
    assert!(trench < sample(&p, &g, -200_000.0, 0.0) - 2000.0);
    assert!(sample(&p, &g, 35_000.0, 0.0) > 500.0);
    assert!(sample(&p, &g, 110_000.0, 0.0) > sample(&p, &g, 35_000.0, 0.0) + 3000.0);
    assert!(sample(&p, &g, 110_000.0, 0.0) > sample(&p, &g, 200_000.0, 0.0) + 5000.0);
    for z in [-15_000.0, 0.0, 15_000.0] {
        assert!(sample(&p, &g, 110_000.0, z) > sample(&p, &g, -110_000.0, z));
    }
    assert!(!p.boundaries[0].backarc_extension);
    assert_eq!(p.structures[0].backarc_offset_m, 0.0);
    evidence("T02_ocean_continent", &g, &p);
}

#[test]
fn t03_ocean_ocean_older_slab_consistent_emergent_island_arc() {
    let f = Fixture::pair(
        false,
        false,
        Point {
            x_m: -40_000.0,
            z_m: 0.0,
        },
    );
    let g = f.grid.clone();
    let p = f.run();
    assert_eq!(p.boundaries[0].subducting_plate, Some(0));
    for z in [-15_000.0, 0.0, 15_000.0] {
        assert!(sample(&p, &g, 110_000.0, z) > 0.0);
        assert!(sample(&p, &g, -35_000.0, z) < sample(&p, &g, -200_000.0, z) - 2000.0);
        assert!(sample(&p, &g, -110_000.0, z) < 0.0);
    }
    assert!(p.oceanic.iter().all(|v| *v)); // Island elevation does not turn basalt into continental crust.
    evidence("T03_island_arc", &g, &p);
    let mut retreat = Fixture::pair(
        false,
        false,
        Point {
            x_m: -40_000.0,
            z_m: 0.0,
        },
    );
    for plate in &mut retreat.plates {
        plate.velocity_history[0].velocity_m_per_ma.x_m += 40_000.0;
    }
    retreat.boundaries = vec![classify_segment(
        0,
        0,
        1,
        retreat.boundaries[0].points.clone(),
        &retreat.plates,
        &retreat.grid,
        &retreat.crust,
    )];
    let retreat = retreat.run();
    assert!(retreat.boundaries[0].backarc_extension);
    assert!(sample(&retreat, &g, 200_000.0, 0.0) < sample(&p, &g, 200_000.0, 0.0) - 500.0);
    assert_eq!(
        retreat.boundaries[0].subducting_plate,
        p.boundaries[0].subducting_plate
    );
    evidence("T03_retreat_backarc", &g, &retreat);
}

#[test]
fn t04_spreading_young_raised_floor_and_age_sequence() {
    let f = Fixture::pair(
        false,
        false,
        Point {
            x_m: 40_000.0,
            z_m: 0.0,
        },
    );
    let g = f.grid.clone();
    let p = f.run();
    assert_eq!(p.boundaries[0].regime, BoundaryRegime::OceanicSpreading);
    for sign in [-1.0, 1.0] {
        let mut previous_age = -1.0;
        let mut previous_height = f64::INFINITY;
        for x in [0.0, 20_000.0, 80_000.0, 200_000.0, 400_000.0] {
            let i = g.index_at(x * sign, 0.0);
            let c = &p.crust[i];
            assert!(c.ocean_floor_age_ma > previous_age);
            assert!(p.elevation_m[i] < previous_height);
            assert_eq!(c.basement_age_ma, c.ocean_floor_age_ma);
            previous_age = c.ocean_floor_age_ma;
            previous_height = p.elevation_m[i];
        }
    }
    assert!(sample(&p, &g, 0.0, 0.0) > sample(&p, &g, 400_000.0, 0.0) + 1500.0);
    evidence("T04_spreading", &g, &p);
}

#[test]
fn t05_continental_rift_graben_shoulders_not_mature_ocean() {
    let f = Fixture::pair(
        true,
        true,
        Point {
            x_m: 40_000.0,
            z_m: 0.0,
        },
    );
    let g = f.grid.clone();
    let p = f.run();
    assert_eq!(p.boundaries[0].regime, BoundaryRegime::ContinentalRift);
    assert!(sample(&p, &g, 0.0, 0.0) < sample(&p, &g, 70_000.0, 0.0) - 1000.0);
    assert!(sample(&p, &g, -70_000.0, 0.0) > sample(&p, &g, -300_000.0, 0.0) + 400.0);
    assert!(p.oceanic.iter().all(|v| !*v));
    evidence("T05_rift", &g, &p);
}

#[test]
fn t06_straight_transform_valley_and_displaced_geological_band_not_wall() {
    let mut f = Fixture::pair(
        true,
        true,
        Point {
            x_m: 0.0,
            z_m: 40_000.0,
        },
    );
    let g = f.grid.clone();
    let (material, _) = inherited_material_point(
        &f.boundaries,
        Point {
            x_m: 50_000.0,
            z_m: 0.0,
        },
    );
    let mut province = ancient(500.0);
    province.footprint.center = material;
    province.footprint.radius_x_m = 100_000.0;
    province.footprint.radius_z_m = 50_000.0;
    province.initial_relief_m = 0.0;
    province.sediment_cover_m = 0.0;
    f.provinces.push(province.clone());
    let p = f.run();
    assert_eq!(p.boundaries[0].regime, BoundaryRegime::Transform);
    assert!(p.elevation_m.iter().all(|h| *h <= 250.0));
    assert!(sample(&p, &g, 0.0, 0.0) < sample(&p, &g, 150_000.0, 0.0));
    assert_eq!(p.crust[g.index_at(50_000.0, 0.0)].lithology_id, 2);
    let mut inactive = Fixture::pair(true, true, Point { x_m: 0.0, z_m: 0.0 });
    inactive.provinces.push(province);
    let inactive = inactive.run();
    assert_eq!(inactive.crust[g.index_at(50_000.0, 0.0)].lithology_id, 1);
    assert!(p.erodibility[g.index_at(0.0, 0.0)] > p.erodibility[g.index_at(200_000.0, 0.0)]);
    evidence("T06_transform", &g, &p);
    evidence("T06_unsheared_band", &g, &inactive);
}

fn bent(mirror: bool, reverse_shear: bool) -> Fixture {
    let mut f = Fixture::pair(
        true,
        true,
        Point {
            x_m: 0.0,
            z_m: if reverse_shear { -40_000.0 } else { 40_000.0 },
        },
    );
    f.grid.rows = 101;
    f.grid.min_z_m = -252_500.0;
    f.grid.depth_m = 505_000.0;
    f.crust = vec![f.crust[0].clone(); f.grid.len()];
    let sign = if mirror { -1.0 } else { 1.0 };
    let points = vec![
        Point {
            x_m: 0.0,
            z_m: -250_000.0,
        },
        Point {
            x_m: 0.0,
            z_m: -150_000.0,
        },
        Point {
            x_m: 100_000.0 * sign,
            z_m: 150_000.0,
        },
        Point {
            x_m: 0.0,
            z_m: 250_000.0,
        },
    ];
    f.ids = (0..f.grid.len())
        .map(|i| usize::from(nearest(&points, point(&f.grid, i)).1 > 0.0))
        .collect();
    f.boundaries = vec![classify_segment(
        0, 0, 1, points, &f.plates, &f.grid, &f.crust,
    )];
    f
}
#[test]
fn t07_releasing_restraining_mirror_and_shear_reverse() {
    let f = bent(false, false);
    let g = f.grid.clone();
    let p = f.run();
    let m = bent(true, false).run();
    let r = bent(false, true).run();
    let mr = bent(true, true).run();
    assert_eq!(p.boundaries[0].regime, BoundaryRegime::Transform);
    let i = g.index_at(0.0, -150_000.0);
    // Compare to the 150m straight-transform valley, isolating the bend effect.
    assert!(p.elevation_m[i] > 200.0);
    assert!(m.elevation_m[i] < 100.0);
    assert!(r.elevation_m[i] < 100.0);
    assert!(mr.elevation_m[i] > 200.0);
    for (name, plan) in [
        ("T07_restraining", &p),
        ("T07_releasing_mirror", &m),
        ("T07_releasing_reverse", &r),
        ("T07_restraining_mirror_reverse", &mr),
    ] {
        evidence(name, &g, plan);
    }
}

#[test]
fn t08_ancient_orogen_age_changes_height_without_active_boundary() {
    let mut young = Fixture::single(true);
    young.provinces.push(ancient(100.0));
    let g = young.grid.clone();
    let young = young.run();
    let mut old = Fixture::single(true);
    old.provinces.push(ancient(1000.0));
    let old = old.run();
    let plain = Fixture::single(true).run();
    let center = g.index_at(0.0, 0.0);
    assert!(young.elevation_m[center] > old.elevation_m[center] + 1000.0);
    assert!(old.elevation_m[center] > plain.elevation_m[center] + 500.0);
    assert!(old.elevation_m[center] > sample(&old, &g, 250_000.0, 0.0) + 500.0);
    assert!(old.boundaries.is_empty());
    assert_eq!(old.crust[center].lithology_id, 2);
    assert!(old.erodibility[center] > plain.erodibility[center]);
    evidence("T08_ancient_young", &g, &young);
    evidence("T08_ancient_old", &g, &old);
}

#[test]
fn t09_hotspot_chain_velocity_change_changes_track_and_age_spacing() {
    let mut f = Fixture::single(false);
    f.grid.rows = 101;
    f.grid.min_z_m = -252_500.0;
    f.grid.depth_m = 505_000.0;
    f.ids = vec![0; f.grid.len()];
    f.crust = vec![f.crust[0].clone(); f.grid.len()];
    let g = f.grid.clone();
    f.plates[0].velocity_history = vec![
        VelocityEpoch {
            start_age_ma: 0.0,
            velocity_m_per_ma: Point {
                x_m: 10_000.0,
                z_m: 0.0,
            },
        },
        VelocityEpoch {
            start_age_ma: 10.0,
            velocity_m_per_ma: Point {
                x_m: 0.0,
                z_m: 20_000.0,
            },
        },
    ];
    f.hotspots.push(Hotspot {
        id: 0,
        plate_id: 0,
        source: Point { x_m: 0.0, z_m: 0.0 },
        duration_ma: 20.0,
        interval_ma: 5.0,
        radius_m: 20_000.0,
        initial_height_m: 7000.0,
    });
    let mut constant = Fixture::single(false);
    constant.grid = f.grid.clone();
    constant.ids = f.ids.clone();
    constant.crust = f.crust.clone();
    constant.plates[0].velocity_history = vec![f.plates[0].velocity_history[0].clone()];
    constant.hotspots = f.hotspots.clone();
    let p = f.run();
    let constant = constant.run();
    for w in p.volcanoes.windows(2) {
        assert!(w[1].age_ma > w[0].age_ma);
        assert!(w[1].height_m < w[0].height_m);
        assert!(w[1].subsidence_m > w[0].subsidence_m);
    }
    assert_eq!(
        p.volcanoes[2].center,
        Point {
            x_m: 100_000.0,
            z_m: 0.0
        }
    );
    assert_eq!(
        p.volcanoes[3].center,
        Point {
            x_m: 100_000.0,
            z_m: 100_000.0
        }
    );
    assert_eq!(
        p.volcanoes[4].center,
        Point {
            x_m: 100_000.0,
            z_m: 200_000.0
        }
    );
    assert_eq!(
        constant.volcanoes[4].center,
        Point {
            x_m: 200_000.0,
            z_m: 0.0
        }
    );
    assert!(sample(&p, &g, 200_000.0, 0.0) < sample(&constant, &g, 200_000.0, 0.0) - 1000.0);
    assert!(sample(&p, &g, 0.0, 0.0) > 0.0);
    evidence("T09_changing_velocity", &g, &p);
    evidence("T09_constant_velocity", &g, &constant);
}

fn equal_age_oceanic_pair() -> Fixture {
    let mut f = Fixture::pair(
        false,
        false,
        Point {
            x_m: -40_000.0,
            z_m: 0.0,
        },
    );
    for c in &mut f.crust {
        c.ocean_floor_age_ma = 120.0;
        c.basement_age_ma = 120.0;
    }
    let points = f.boundaries[0].points.clone();
    f.boundaries = vec![classify_segment(
        0, 0, 1, points, &f.plates, &f.grid, &f.crust,
    )];
    f
}

fn relabeled(mut f: Fixture) -> Fixture {
    let rename = |id| if id == 0 { 71 } else { 8 };
    for p in &mut f.plates {
        p.id = rename(p.id);
    }
    f.plates.reverse();
    for id in &mut f.ids {
        *id = rename(*id);
    }
    for b in &mut f.boundaries {
        // Exchange the actual sides as well as changing arbitrary numeric IDs.
        let mut points = b.points.clone();
        points.reverse();
        *b = classify_segment(
            b.id,
            rename(b.plate_b),
            rename(b.plate_a),
            points,
            &f.plates,
            &f.grid,
            &f.crust,
        );
    }
    f
}
#[test]
fn t10_plate_id_permutation_and_side_exchange_preserve_physics() {
    for (name, fixture, renamed) in [
        (
            "T10_subduction",
            Fixture::pair(
                false,
                true,
                Point {
                    x_m: -40_000.0,
                    z_m: 0.0,
                },
            ),
            relabeled(Fixture::pair(
                false,
                true,
                Point {
                    x_m: -40_000.0,
                    z_m: 0.0,
                },
            )),
        ),
        (
            "T10_oceanic",
            Fixture::pair(
                false,
                false,
                Point {
                    x_m: -40_000.0,
                    z_m: 0.0,
                },
            ),
            relabeled(Fixture::pair(
                false,
                false,
                Point {
                    x_m: -40_000.0,
                    z_m: 0.0,
                },
            )),
        ),
        (
            "T10_equal_age_oceanic",
            equal_age_oceanic_pair(),
            relabeled(equal_age_oceanic_pair()),
        ),
        (
            "T10_bend",
            bent(false, false),
            relabeled(bent(false, false)),
        ),
    ] {
        let g = fixture.grid.clone();
        let original_partition = partition(&g, &fixture.plates);
        let relabeled_partition = partition(&g, &renamed.plates);
        assert_eq!(
            original_partition
                .iter()
                .map(|id| if *id == 0 { 71 } else { 8 })
                .collect::<Vec<_>>(),
            relabeled_partition
        );
        let a = fixture.run();
        let b = renamed.run();
        assert_eq!(a.boundaries[0].regime, b.boundaries[0].regime);
        assert_eq!(a.oceanic, b.oceanic);
        assert_eq!(a.crust, b.crust);
        assert_eq!(a.elevation_m, b.elevation_m);
        assert_eq!(a.erodibility, b.erodibility);
        evidence(name, &g, &a);
        evidence(&format!("{name}_permuted"), &g, &b);
    }
}

#[test]
fn physical_partition_is_connected_and_crust_does_not_follow_plate_ids() {
    let g = Grid {
        columns: 41,
        rows: 31,
        min_x_m: 0.0,
        min_z_m: 0.0,
        width_m: 20_500_000.0,
        depth_m: 15_500_000.0,
        step_m: 500_000.0,
    };
    for seed in [0, 1, -1, 42, 712816, i64::MIN, i64::MAX] {
        let p = build(&g, seed).unwrap();
        p.validate(&g).unwrap();
        assert_eq!(p, build(&g, seed).unwrap());
        assert!(p.plates.iter().any(|plate| {
            let mut land = false;
            let mut ocean = false;
            for (i, id) in p.plate_id.iter().enumerate() {
                if *id == plate.id {
                    land |= !p.oceanic[i];
                    ocean |= p.oceanic[i];
                }
            }
            land && ocean
        }));
        assert!(p.cold_phase.iter().any(|v| *v));
        evidence(&format!("tectonics_seed_{seed}"), &g, &p);
    }
    let tiny = Grid {
        columns: 5,
        rows: 5,
        min_x_m: 0.0,
        min_z_m: 0.0,
        width_m: 25_000.0,
        depth_m: 25_000.0,
        step_m: 5000.0,
    };
    assert_eq!(build(&tiny, 42).unwrap().plates.len(), 1);
}

#[test]
fn small_domains_observe_large_crust_and_inherited_footprints_without_rescaling() {
    let tiny_grid = Grid {
        columns: 16,
        rows: 16,
        min_x_m: -8000.0,
        min_z_m: -8000.0,
        width_m: 16_000.0,
        depth_m: 16_000.0,
        step_m: 1000.0,
    };
    let wider_grid = Grid {
        columns: 64,
        rows: 64,
        min_x_m: -320_000.0,
        min_z_m: -320_000.0,
        width_m: 640_000.0,
        depth_m: 640_000.0,
        step_m: 10_000.0,
    };
    let tiny = build(&tiny_grid, 42).unwrap();
    let wider = build(&wider_grid, 42).unwrap();
    // Both are crops inside one nominal physical support area. Material age,
    // province shape, and continental scale must not shrink to the small window.
    assert_eq!(tiny.fragments, wider.fragments);
    assert_eq!(tiny.provinces, wider.provinces);
    assert!(
        tiny.fragments
            .iter()
            .all(|f| f.footprint.radius_x_m > tiny_grid.width_m
                && f.footprint.radius_z_m > tiny_grid.depth_m)
    );
    evidence("tectonics_small_physical_crop", &tiny_grid, &tiny);
    evidence("tectonics_wider_physical_crop", &wider_grid, &wider);
}

fn enclosed_plate(relative_speed_m_per_ma: f64) -> Fixture {
    let mut f = Fixture::pair(
        false,
        false,
        Point {
            x_m: relative_speed_m_per_ma,
            z_m: 0.0,
        },
    );
    f.grid = Grid {
        columns: 81,
        rows: 81,
        min_x_m: -405_000.0,
        min_z_m: -405_000.0,
        width_m: 810_000.0,
        depth_m: 810_000.0,
        step_m: 10_000.0,
    };
    f.plates[0].nucleus = Point {
        x_m: -250_000.0,
        z_m: 0.0,
    };
    f.plates[1].nucleus = Point { x_m: 0.0, z_m: 0.0 };
    f.ids = (0..f.grid.len())
        .map(|i| {
            let p = point(&f.grid, i);
            usize::from(p.x_m.abs() <= 100_000.0 && p.z_m.abs() <= 100_000.0)
        })
        .collect();
    f.crust = vec![f.crust[0].clone(); f.grid.len()];
    f.boundaries = extract_boundaries(&f.grid, &f.ids, &f.plates, &f.crust);
    f
}

#[test]
fn enclosed_translating_plate_extraction_preserves_opposing_local_regimes_and_relief() {
    let moving = enclosed_plate(40_000.0);
    let g = moving.grid.clone();
    let moving = moving.run();
    let stationary = enclosed_plate(0.0).run();
    let east = moving
        .boundaries
        .iter()
        .min_by(|a, b| {
            let p = Point {
                x_m: 105_000.0,
                z_m: 0.0,
            };
            nearest(&a.points, p).0.total_cmp(&nearest(&b.points, p).0)
        })
        .unwrap();
    let west = moving
        .boundaries
        .iter()
        .min_by(|a, b| {
            let p = Point {
                x_m: -105_000.0,
                z_m: 0.0,
            };
            nearest(&a.points, p).0.total_cmp(&nearest(&b.points, p).0)
        })
        .unwrap();
    assert_eq!(east.regime, BoundaryRegime::Subduction);
    assert_eq!(west.regime, BoundaryRegime::OceanicSpreading);
    assert!(east.normal_motion_m_per_ma < -MOTION_EPSILON_M_PER_MA);
    assert!(west.normal_motion_m_per_ma > MOTION_EPSILON_M_PER_MA);
    assert!(
        moving
            .boundaries
            .iter()
            .all(|b| b.regime != BoundaryRegime::Passive)
    );
    for arc in moving
        .boundaries
        .iter()
        .filter(|b| b.regime == BoundaryRegime::Subduction)
    {
        assert_eq!(arc.subducting_plate, Some(0));
    }
    assert!(
        sample(&moving, &g, -110_000.0, 0.0) > sample(&stationary, &g, -110_000.0, 0.0) + 2500.0
    );
    assert!(sample(&moving, &g, 0.0, 0.0) > sample(&stationary, &g, 0.0, 0.0) + 3000.0);
    assert!(sample(&moving, &g, 140_000.0, 0.0) < sample(&moving, &g, 250_000.0, 0.0) - 1500.0);
    evidence("tectonics_enclosed_translating_plate", &g, &moving);
    evidence("tectonics_enclosed_stationary_plate", &g, &stationary);
}

#[test]
fn persisted_plan_rejects_unknown_ids_nonfinite_and_disconnected_plates() {
    let f = Fixture::pair(
        true,
        true,
        Point {
            x_m: -40_000.0,
            z_m: 0.0,
        },
    );
    let g = f.grid.clone();
    let p = f.run();
    let mut broken = p.clone();
    broken.elevation_m[0] = f64::NAN;
    assert!(broken.validate(&g).is_err());
    let mut broken = p.clone();
    broken.crust[0].lithology_id = usize::MAX;
    assert!(broken.validate(&g).is_err());
    let mut broken = p.clone();
    broken.plate_id[0] = 1;
    assert!(broken.validate(&g).is_err());
    let mut broken = p.clone();
    broken.boundaries[0].subducting_plate = Some(999);
    assert!(broken.validate(&g).is_err());
    let bytes = serde_json::to_vec(&p).unwrap();
    let restored: TectonicPlan = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(p, restored);
    restored.validate(&g).unwrap();
    let mut unknown = serde_json::to_value(&p).unwrap();
    unknown
        .as_object_mut()
        .unwrap()
        .insert("unowned_field".into(), serde_json::json!(true));
    assert!(serde_json::from_value::<TectonicPlan>(unknown).is_err());
    let mut missing_chain = Fixture::single(false);
    missing_chain.hotspots.push(Hotspot {
        id: 0,
        plate_id: 0,
        source: Point { x_m: 0.0, z_m: 0.0 },
        duration_ma: 10.0,
        interval_ma: 5.0,
        radius_m: 20_000.0,
        initial_height_m: 7000.0,
    });
    let mut missing_chain = missing_chain.run();
    missing_chain.volcanoes.pop();
    assert!(missing_chain.validate(&g).is_err());
}
