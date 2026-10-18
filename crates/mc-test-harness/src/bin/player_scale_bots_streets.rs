/// Authored city street routes; movement still requires fresh received terrain.
#[derive(Debug, Default)]
struct StreetRoutes(Vec<Vec<[i32; 3]>>);

impl StreetRoutes {
    fn load(path: &std::path::Path, clients: u32) -> Result<Self> {
        let routes: Vec<Vec<[i32; 3]>> =
            serde_json::from_reader(std::fs::File::open(path)?).context("load street routes")?;
        ensure!(
            routes.len() == clients as usize,
            "street routes must name every client exactly once"
        );
        for route in &routes {
            ensure!(
                (2..=4096).contains(&route.len()),
                "street route needs 2..=4096 waypoints"
            );
            for (point, next) in route.iter().zip(route.iter().cycle().skip(1)) {
                let dx = i64::from(point[0]) - i64::from(next[0]);
                let dz = i64::from(point[2]) - i64::from(next[2]);
                let dy = i64::from(point[1]) - i64::from(next[1]);
                ensure!(
                    dx.abs() + dz.abs() == 1 && dy.abs() <= 1,
                    "street route edges must be cardinal neighbors with <=1 block height change"
                );
                ensure!(
                    (MIN_Y..MIN_Y + HEIGHT as i32).contains(&point[1]),
                    "street route height outside overworld"
                );
            }
        }
        Ok(Self(routes))
    }
}

impl Bot {
    fn street_step(&mut self, speed: f64) -> Option<(f64, f64, f32)> {
        let route = self.street_routes.0.get(self.index as usize)?;
        let target = route[self.street_waypoint];
        let (tx, tz) = (f64::from(target[0]) + 0.5, f64::from(target[2]) + 0.5);
        if (self.x - tx).abs() < 0.001 && (self.z - tz).abs() < 0.001 {
            self.street_waypoint = (self.street_waypoint + 1) % route.len();
            self.metrics.street_waypoints_reached += 1;
        }
        let target = route[self.street_waypoint];
        let (dx, dz) = (
            f64::from(target[0]) + 0.5 - self.x,
            f64::from(target[2]) + 0.5 - self.z,
        );
        // A correction off the authored edge is not permission to take an
        // unvalidated diagonal shortcut across a courtyard or building.
        if dx.abs() > 0.001 && dz.abs() > 0.001 {
            self.metrics.route_refusals_not_walkable += 1;
            return Some((0.0, 0.0, self.yaw));
        }
        let distance = dx.abs() + dz.abs();
        if distance < 0.001 {
            return Some((0.0, 0.0, self.yaw));
        }
        let factor = speed.min(distance) / distance;
        let yaw = if dx > 0.001 {
            -90.0
        } else if dx < -0.001 {
            90.0
        } else if dz > 0.0 {
            0.0
        } else {
            180.0
        };
        Some((dx * factor, dz * factor, yaw))
    }
}
