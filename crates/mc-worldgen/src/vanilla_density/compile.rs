use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{
    Binary, BlendedNoise, DensityRuntime, Id, Mapped, Marker, Node, Shift, Spline, SplinePoint,
};
use crate::vanilla_climate::noise::NoiseFactory;

pub(super) fn load(seed: u64, root: &Path) -> Result<DensityRuntime, String> {
    let root = if root.join("data/minecraft/worldgen").is_dir() {
        root.join("data")
    } else {
        root.to_path_buf()
    };
    let settings = read_json(&root.join("minecraft/worldgen/noise_settings/overworld.json"))?;
    let noise = get(&settings, "noise")?;
    let mut compiler = Compiler {
        root,
        factory: NoiseFactory::new(seed),
        runtime: DensityRuntime {
            nodes: Vec::new(),
            noises: Vec::new(),
            blended: Vec::new(),
            fields: HashMap::new(),
            min_y: integer(noise, "min_y")?,
            height: integer(noise, "height")?,
            sea_level: integer(&settings, "sea_level")?,
            cell_width: integer(noise, "size_horizontal")? * 4,
            cell_height: integer(noise, "size_vertical")? * 4,
        },
        interned: HashMap::new(),
        references: HashMap::new(),
        loading: HashSet::new(),
        noises: HashMap::new(),
    };
    if compiler.runtime.cell_width <= 0
        || compiler.runtime.cell_height <= 0
        || compiler.runtime.height <= 0
    {
        return Err("Invalid overworld noise cell geometry".into());
    }
    for (name, value) in get(&settings, "noise_router")?
        .as_object()
        .ok_or("noise_router must be an object")?
    {
        let id = compiler.compile(value)?;
        compiler.runtime.fields.insert(name.clone(), id);
    }
    let cheese = compiler.reference("minecraft:overworld/sloped_cheese")?;
    compiler
        .runtime
        .fields
        .insert("sloped_cheese".into(), cheese);
    if let Some(surface) = get(&settings, "noise_router")?
        .get("final_density")
        .and_then(lod_surface_json)
    {
        let id = compiler.compile(&surface)?;
        compiler.runtime.fields.insert("lod_surface".into(), id);
    }
    Ok(compiler.runtime)
}

/// Far-terrain LOD surface: the argument of the final density's first
/// `interpolated` wrapper (slide and blending included) with the cave branch
/// (`range_choice` on the sloped cheese) replaced by the sloped cheese itself.
/// `None` when the router does not have that shape.
pub(super) fn lod_surface_json(final_density: &Value) -> Option<Value> {
    fn interpolated(value: &Value) -> Option<&Value> {
        let object = value.as_object()?;
        if object.get("type")?.as_str()? == "minecraft:interpolated" {
            return object.get("argument");
        }
        object.values().find_map(interpolated)
    }
    fn without_caves(value: &mut Value) -> bool {
        const CHEESE: &str = "minecraft:overworld/sloped_cheese";
        let Some(object) = value.as_object_mut() else {
            return false;
        };
        if object.get("type").and_then(Value::as_str) == Some("minecraft:range_choice")
            && object.get("input").and_then(Value::as_str) == Some(CHEESE)
        {
            *value = Value::String(CHEESE.into());
            return true;
        }
        let mut replaced = false;
        for child in object.values_mut() {
            replaced |= without_caves(child);
        }
        replaced
    }
    let mut surface = interpolated(final_density)?.clone();
    without_caves(&mut surface).then_some(surface)
}

struct Compiler {
    root: PathBuf,
    factory: NoiseFactory,
    runtime: DensityRuntime,
    interned: HashMap<String, Id>,
    references: HashMap<String, Id>,
    loading: HashSet<String>,
    noises: HashMap<String, usize>,
}

impl Compiler {
    fn reference(&mut self, name: &str) -> Result<Id, String> {
        if let Some(&id) = self.references.get(name) {
            return Ok(id);
        }
        if !self.loading.insert(name.into()) {
            return Err(format!("Cyclic density reference {name}"));
        }
        let value = read_json(&resource_path(&self.root, "density_function", name)?)?;
        let id = self
            .compile(&value)
            .map_err(|error| format!("{name}: {error}"))?;
        self.loading.remove(name);
        self.references.insert(name.into(), id);
        Ok(id)
    }

    fn noise(&mut self, value: &Value, field: &str) -> Result<usize, String> {
        let name = get(value, field)?
            .as_str()
            .ok_or_else(|| format!("{field} must name a noise resource"))?;
        if let Some(&id) = self.noises.get(name) {
            return Ok(id);
        }
        let parameters = read_json(&resource_path(&self.root, "noise", name)?)?;
        let first = integer(&parameters, "firstOctave")?;
        let amplitudes = get(&parameters, "amplitudes")?
            .as_array()
            .ok_or("Noise amplitudes must be an array")?
            .iter()
            .map(|value| {
                value
                    .as_f64()
                    .ok_or_else(|| "Noise amplitude must be numeric".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if amplitudes.is_empty() {
            return Err(format!("Noise {name} has no amplitudes"));
        }
        let id = self.runtime.noises.len();
        self.runtime
            .noises
            .push(self.factory.normal(name, first, &amplitudes));
        self.noises.insert(name.into(), id);
        Ok(id)
    }

    fn child(&mut self, value: &Value, field: &str) -> Result<Id, String> {
        self.compile(get(value, field)?)
    }

    fn compile(&mut self, value: &Value) -> Result<Id, String> {
        if let Some(name) = value.as_str() {
            return self.reference(name);
        }
        let key = value.to_string();
        if let Some(&id) = self.interned.get(&key) {
            return Ok(id);
        }
        let node = if let Some(number) = value.as_f64() {
            Node::Constant(number)
        } else {
            let kind = get(value, "type")?
                .as_str()
                .ok_or("Density type must be a string")?
                .strip_prefix("minecraft:")
                .ok_or("Only minecraft density operators are supported")?;
            match kind {
                "constant" => Node::Constant(number(value, "argument")?),
                "add" | "mul" | "min" | "max" => Node::Binary(
                    match kind {
                        "add" => Binary::Add,
                        "mul" => Binary::Mul,
                        "min" => Binary::Min,
                        _ => Binary::Max,
                    },
                    self.child(value, "argument1")?,
                    self.child(value, "argument2")?,
                ),
                "abs" | "square" | "cube" | "half_negative" | "quarter_negative" | "invert"
                | "squeeze" => Node::Mapped(
                    match kind {
                        "abs" => Mapped::Abs,
                        "square" => Mapped::Square,
                        "cube" => Mapped::Cube,
                        "half_negative" => Mapped::HalfNegative,
                        "quarter_negative" => Mapped::QuarterNegative,
                        "invert" => Mapped::Invert,
                        _ => Mapped::Squeeze,
                    },
                    self.child(value, "argument")?,
                ),
                "clamp" => Node::Clamp(
                    self.child(value, "input")?,
                    number(value, "min")?,
                    number(value, "max")?,
                ),
                "y_clamped_gradient" => {
                    let from_y = integer(value, "from_y")?;
                    let to_y = integer(value, "to_y")?;
                    // Vanilla's YClampedGradient divides by (toY - fromY): reject a
                    // degenerate range whose evaluator would divide by zero and emit NaN
                    // instead of a gradient. Today's data (29 sites) has none, so honest
                    // rejection costs nothing on the supported graph.
                    if from_y == to_y {
                        return Err(format!(
                            "y_clamped_gradient needs from_y != to_y (got {from_y})"
                        ));
                    }
                    Node::Gradient(
                        from_y,
                        to_y,
                        number(value, "from_value")?,
                        number(value, "to_value")?,
                    )
                }
                "noise" => Node::Noise(
                    self.noise(value, "noise")?,
                    number(value, "xz_scale")?,
                    number(value, "y_scale")?,
                ),
                "shift" | "shift_a" | "shift_b" => Node::Shift(
                    self.noise(value, "argument")?,
                    match kind {
                        "shift_a" => Shift::A,
                        "shift_b" => Shift::B,
                        _ => Shift::All,
                    },
                ),
                "shifted_noise" => Node::Shifted(
                    self.noise(value, "noise")?,
                    self.child(value, "shift_x")?,
                    self.child(value, "shift_y")?,
                    self.child(value, "shift_z")?,
                    number(value, "xz_scale")?,
                    number(value, "y_scale")?,
                ),
                "weird_scaled_sampler" => {
                    let mapper = get(value, "rarity_value_mapper")?
                        .as_str()
                        .ok_or("Invalid cave rarity mapper")?;
                    if !matches!(mapper, "type_1" | "type_2") {
                        return Err(format!("Unknown cave rarity mapper {mapper}"));
                    }
                    Node::Weird(
                        self.noise(value, "noise")?,
                        self.child(value, "input")?,
                        mapper == "type_2",
                    )
                }
                "range_choice" => Node::Range(
                    self.child(value, "input")?,
                    number(value, "min_inclusive")?,
                    number(value, "max_exclusive")?,
                    self.child(value, "when_in_range")?,
                    self.child(value, "when_out_of_range")?,
                ),
                "spline" => Node::Spline(self.spline(get(value, "spline")?)?),
                "interpolated" | "flat_cache" | "cache_2d" | "cache_once" | "cache_all_in_cell"
                | "blend_density" => Node::Marker(
                    match kind {
                        "interpolated" => Marker::Interpolated,
                        "flat_cache" => Marker::FlatCache,
                        "cache_2d" => Marker::Cache2D,
                        "cache_once" => Marker::CacheOnce,
                        "cache_all_in_cell" => Marker::CacheAllInCell,
                        _ => Marker::Pass,
                    },
                    self.child(value, "argument")?,
                ),
                "blend_alpha" => Node::Constant(1.0),
                "blend_offset" | "beardifier" => Node::Constant(0.0),
                "old_blended_noise" => {
                    let id = self.runtime.blended.len();
                    self.runtime.blended.push(BlendedNoise::new(
                        &self.factory,
                        number(value, "xz_scale")?,
                        number(value, "y_scale")?,
                        number(value, "xz_factor")?,
                        number(value, "y_factor")?,
                        number(value, "smear_scale_multiplier")?,
                    ));
                    Node::Blended(id)
                }
                "find_top_surface" => {
                    let step = integer(value, "cell_height")?;
                    // Vanilla's FindTopSurface steps by cell_height: a non-positive
                    // step would loop forever or never advance. Today's data uses 8
                    // (3 sites), so honest rejection costs nothing here.
                    if step <= 0 {
                        return Err(format!(
                            "find_top_surface needs cell_height > 0 (got {step})"
                        ));
                    }
                    Node::FindTop(
                        self.child(value, "density")?,
                        self.child(value, "upper_bound")?,
                        integer(value, "lower_bound")?,
                        step,
                    )
                }
                _ => return Err(format!("Unsupported density operator {kind}")),
            }
        };
        let id = self.runtime.nodes.len();
        self.runtime.nodes.push(node);
        self.interned.insert(key, id);
        Ok(id)
    }

    fn spline(&mut self, value: &Value) -> Result<Spline, String> {
        if let Some(number) = value.as_f64() {
            return Ok(Spline::Constant(number as f32));
        }
        let coordinate = self.child(value, "coordinate")?;
        let mut points = Vec::new();
        for point in get(value, "points")?
            .as_array()
            .ok_or("Spline points must be an array")?
        {
            let location = number(point, "location")? as f32;
            if points
                .last()
                .is_some_and(|point: &SplinePoint| point.location >= location)
            {
                return Err("Spline locations must increase".into());
            }
            points.push(SplinePoint {
                location,
                value: self.spline(get(point, "value")?)?,
                derivative: number(point, "derivative")? as f32,
            });
        }
        if points.is_empty() {
            return Err("Spline requires at least one point".into());
        }
        Ok(Spline::Multipoint { coordinate, points })
    }
}

fn resource_path(root: &Path, registry: &str, name: &str) -> Result<PathBuf, String> {
    let (namespace, path) = name.split_once(':').unwrap_or(("minecraft", name));
    if namespace != "minecraft"
        || path
            .split('/')
            .any(|part| part.is_empty() || part == ".." || part == ".")
    {
        return Err(format!("Invalid vanilla resource {name}"));
    }
    // Keep reads inside the content root: reject absolute paths and Windows
    // drive prefixes that the segment walk above would otherwise accept.
    if path.starts_with('/') || path.as_bytes().contains(&b'\\') {
        return Err(format!("Invalid vanilla resource {name}"));
    }
    Ok(root
        .join(namespace)
        .join("worldgen")
        .join(registry)
        .join(format!("{path}.json")))
}
fn read_json(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?)
        .map_err(|error| format!("{}: {error}", path.display()))
}
fn get<'a>(value: &'a Value, field: &str) -> Result<&'a Value, String> {
    value
        .get(field)
        .ok_or_else(|| format!("Missing field {field}"))
}
fn number(value: &Value, field: &str) -> Result<f64, String> {
    get(value, field)?
        .as_f64()
        .ok_or_else(|| format!("{field} must be numeric"))
}
fn integer(value: &Value, field: &str) -> Result<i32, String> {
    get(value, field)?
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| format!("{field} must be an i32"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn compiler() -> Compiler {
        Compiler {
            root: PathBuf::from("test-content"),
            factory: NoiseFactory::new(0),
            runtime: DensityRuntime {
                nodes: Vec::new(),
                noises: Vec::new(),
                blended: Vec::new(),
                fields: HashMap::new(),
                min_y: -64,
                height: 384,
                sea_level: 63,
                cell_width: 4,
                cell_height: 8,
            },
            interned: HashMap::new(),
            references: HashMap::new(),
            loading: HashSet::new(),
            noises: HashMap::new(),
        }
    }

    #[test]
    fn gradient_rejects_degenerate_range() {
        let mut compiler = compiler();
        let error = compiler
            .compile(&json!({
                "type": "minecraft:y_clamped_gradient",
                "from_y": 24,
                "to_y": 24,
                "from_value": 0.0,
                "to_value": 1.0,
            }))
            .expect_err("from_y == to_y must not compile");
        assert!(
            error.contains("from_y != to_y"),
            "unexpected error: {error}"
        );
        let id = compiler
            .compile(&json!({
                "type": "minecraft:y_clamped_gradient",
                "from_y": -64,
                "to_y": 320,
                "from_value": 1.5,
                "to_value": -1.5,
            }))
            .expect("unequal endpoints must compile");
        assert!(matches!(
            compiler.runtime.nodes[id],
            Node::Gradient(-64, 320, _, _)
        ));
    }

    #[test]
    fn find_top_rejects_non_positive_step() {
        for step in [0, -8] {
            let mut compiler = compiler();
            let error = compiler
                .compile(&json!({
                    "type": "minecraft:find_top_surface",
                    "density": 0.0,
                    "upper_bound": 0.0,
                    "lower_bound": -64,
                    "cell_height": step,
                }))
                .expect_err("non-positive step must not compile");
            assert!(
                error.contains("cell_height > 0"),
                "unexpected error: {error}"
            );
        }
        let mut compiler = compiler();
        let id = compiler
            .compile(&json!({
                "type": "minecraft:find_top_surface",
                "density": 0.0,
                "upper_bound": 0.0,
                "lower_bound": -64,
                "cell_height": 8,
            }))
            .expect("positive step must compile");
        assert!(matches!(
            compiler.runtime.nodes[id],
            Node::FindTop(_, _, -64, 8)
        ));
    }

    #[test]
    fn resource_path_rejects_escapes() {
        let root = PathBuf::from("test-content");
        for name in [
            "minecraft:/absolute",
            "minecraft:..\\windows",
            "minecraft:overworld\\offset",
            "other:overworld/offset",
            "minecraft:overworld/../offset",
        ] {
            assert!(
                resource_path(&root, "density_function", name).is_err(),
                "{name} must not resolve"
            );
        }
        assert_eq!(
            resource_path(&root, "density_function", "minecraft:overworld/offset"),
            Ok(root
                .join("minecraft")
                .join("worldgen")
                .join("density_function")
                .join("overworld/offset.json"))
        );
    }
}
