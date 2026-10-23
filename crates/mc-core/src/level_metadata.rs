//! Pure Minecraft 26.1 metadata templates derived from the v2 checkpoint.
use std::collections::BTreeMap;

use mc_nbt::{ListTag, Tag, tag_type};

use crate::config::CoreError;
use crate::journal::CheckpointManifest;
use crate::weather::ClimateImage;

type Fields = Vec<(String, Tag)>;

/// Relative Anvil saved-data path to its complete, unknown-field-preserving NBT root.
pub(crate) type MetadataTags = BTreeMap<String, Tag>;

const BORDER_PATHS: [(crate::dimensions::Dimension, &str); 3] = [
    (
        crate::dimensions::Dimension::Overworld,
        "dimensions/minecraft/overworld/data/minecraft/world_border.dat",
    ),
    (
        crate::dimensions::Dimension::Nether,
        "dimensions/minecraft/the_nether/data/minecraft/world_border.dat",
    ),
    (
        crate::dimensions::Dimension::End,
        "dimensions/minecraft/the_end/data/minecraft/world_border.dat",
    ),
];

const TICKET_PATHS: [(crate::dimensions::Dimension, &str); 3] = [
    (
        crate::dimensions::Dimension::Overworld,
        "dimensions/minecraft/overworld/data/minecraft/chunk_tickets.dat",
    ),
    (
        crate::dimensions::Dimension::Nether,
        "dimensions/minecraft/the_nether/data/minecraft/chunk_tickets.dat",
    ),
    (
        crate::dimensions::Dimension::End,
        "dimensions/minecraft/the_end/data/minecraft/chunk_tickets.dat",
    ),
];
const FORCED_TICKET_OWNER: &str = crate::forceload::OWNER_PREFIX;

fn failure(path: &str, detail: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("vanilla metadata {path}: {detail}"))
}

fn field<'a>(fields: &'a [(String, Tag)], key: &str) -> Option<&'a Tag> {
    fields
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

fn put(fields: &mut Fields, key: &str, value: Tag) {
    fields.retain(|(name, _)| name != key);
    fields.push((key.into(), value));
}

/// Validate converter-owned relative paths without consulting the filesystem.
pub(crate) fn validate_relative_path(path: &str) -> Result<(), CoreError> {
    if path.contains(['\\', '\0', ':'])
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(failure(path, "expected safe relative saved-data path"));
    }
    Ok(())
}

pub(crate) fn to_tag(tags: &MetadataTags) -> Tag {
    Tag::Compound(
        tags.iter()
            .map(|(path, tag)| (path.clone(), tag.clone()))
            .collect(),
    )
}

pub(crate) fn from_tag(tag: &Tag) -> Result<MetadataTags, CoreError> {
    let Tag::Compound(fields) = tag else {
        return Err(failure("level_metadata", "expected compound templates"));
    };
    let mut tags = MetadataTags::new();
    for (path, tag) in fields {
        validate_relative_path(path)?;
        if !matches!(tag, Tag::Compound(_)) {
            return Err(failure(path, "expected compound root"));
        }
        if tags.insert(path.clone(), tag.clone()).is_some() {
            return Err(failure(path, "duplicate saved-data path"));
        }
    }
    Ok(tags)
}

fn root_fields<'a>(tags: &'a MetadataTags, path: &str) -> Result<Option<&'a Fields>, CoreError> {
    match tags.get(path) {
        None => Ok(None),
        Some(Tag::Compound(fields)) => Ok(Some(fields)),
        Some(_) => Err(failure(path, "expected compound root")),
    }
}

fn root_fields_mut<'a>(
    tags: &'a mut MetadataTags,
    path: &str,
) -> Result<&'a mut Fields, CoreError> {
    if !tags.contains_key(path) {
        tags.insert(path.into(), Tag::Compound(Vec::new()));
    }
    match tags.get_mut(path) {
        Some(Tag::Compound(fields)) => Ok(fields),
        _ => Err(failure(path, "expected compound root")),
    }
}

fn compound_field_mut<'a>(
    fields: &'a mut Fields,
    key: &str,
    path: &str,
) -> Result<&'a mut Fields, CoreError> {
    let index = match fields.iter().position(|(name, _)| name == key) {
        Some(index) => index,
        None => {
            fields.push((key.into(), Tag::Compound(Vec::new())));
            fields.len() - 1
        }
    };
    match &mut fields[index].1 {
        Tag::Compound(fields) => Ok(fields),
        _ => Err(failure(path, format!("invalid compound {key}"))),
    }
}

fn saved_data<'a>(tags: &'a MetadataTags, path: &str) -> Result<Option<&'a Fields>, CoreError> {
    let Some(root) = root_fields(tags, path)? else {
        return Ok(None);
    };
    match field(root, "data") {
        Some(Tag::Compound(data)) => Ok(Some(data)),
        _ => Err(failure(path, "missing compound data")),
    }
}

fn saved_data_mut<'a>(tags: &'a mut MetadataTags, path: &str) -> Result<&'a mut Fields, CoreError> {
    if tags.contains_key(path) {
        saved_data(tags, path)?;
    }
    let root = root_fields_mut(tags, path)?;
    put(
        root,
        "DataVersion",
        Tag::Int(mc_world::anvil::TARGET_DATA_VERSION),
    );
    compound_field_mut(root, "data", path)
}

/// Import modern saved-data fields without reading or changing their templates.
pub(crate) fn import_tags(
    tags: &MetadataTags,
    manifest: &mut CheckpointManifest,
) -> Result<(), CoreError> {
    let level_path = "level.dat";
    if let Some(level) = root_fields(tags, level_path)? {
        let Some(Tag::Compound(data)) = field(level, "Data") else {
            return Err(failure(level_path, "missing compound Data"));
        };
        manifest.tick = match field(data, "Time") {
            Some(Tag::Long(value)) if *value >= 0 => *value as u64,
            None => 0,
            _ => return Err(failure(level_path, "invalid Data.Time")),
        };
        let Some(Tag::Compound(spawn)) = field(data, "spawn") else {
            return Err(failure(
                level_path,
                "missing compound Data.spawn; legacy SpawnX/SpawnY/SpawnZ are unsupported",
            ));
        };
        if !matches!(field(spawn, "dimension"), Some(Tag::String(value)) if value == "minecraft:overworld")
        {
            return Err(failure(level_path, "invalid Data.spawn.dimension"));
        }
        manifest.world_spawn = match field(spawn, "pos") {
            Some(Tag::IntArray(value)) => Some(
                value
                    .as_slice()
                    .try_into()
                    .map_err(|_| failure(level_path, "invalid Data.spawn.pos length"))?,
            ),
            _ => return Err(failure(level_path, "invalid Data.spawn.pos")),
        };
        let angle = |key| match field(spawn, key) {
            Some(Tag::Float(value)) if value.is_finite() => Ok(*value),
            _ => Err(failure(level_path, format!("invalid Data.spawn.{key}"))),
        };
        manifest.world_spawn_rotation = [angle("yaw")?, angle("pitch")?];
        if let Some(settings) = field(data, "difficulty_settings") {
            let Tag::Compound(settings) = settings else {
                return Err(failure(level_path, "invalid difficulty_settings"));
            };
            manifest.difficulty = match field(settings, "difficulty") {
                Some(Tag::String(value)) if value == "peaceful" => {
                    crate::survival::Difficulty::Peaceful
                }
                Some(Tag::String(value)) if value == "easy" => crate::survival::Difficulty::Easy,
                Some(Tag::String(value)) if value == "normal" => {
                    crate::survival::Difficulty::Normal
                }
                Some(Tag::String(value)) if value == "hard" => crate::survival::Difficulty::Hard,
                _ => {
                    return Err(failure(
                        level_path,
                        "invalid difficulty_settings.difficulty",
                    ));
                }
            };
        }
    }
    let rules_path = "data/minecraft/game_rules.dat";
    if let Some(data) = saved_data(tags, rules_path)? {
        for rule in crate::command_rules::rules() {
            let key = format!("minecraft:{}", rule.name);
            let Some(value) = field(data, &key) else {
                continue;
            };
            let value = match (rule.kind, value) {
                (crate::command_rules::RuleKind::Boolean, Tag::Byte(value))
                    if matches!(value, 0 | 1) =>
                {
                    (*value != 0).to_string()
                }
                (crate::command_rules::RuleKind::Integer { min, max }, Tag::Int(value))
                    if (min..=max).contains(value) =>
                {
                    value.to_string()
                }
                _ => return Err(failure(rules_path, format!("invalid {key}"))),
            };
            manifest.gamerules.insert(rule.name.into(), value);
        }
    }
    let mut climate = ClimateImage {
        day_time: 0,
        weather_kind: 0,
        remaining_ticks: 12000,
        thunder_remaining_ticks: 12000,
        clear_remaining_ticks: 0,
        thundering: false,
        rain_level_bits: 0.0_f32.to_bits(),
        thunder_level_bits: 0.0_f32.to_bits(),
        advance_time: manifest
            .gamerules
            .get("advance_time")
            .is_none_or(|value| value == "true"),
        advance_weather: manifest
            .gamerules
            .get("advance_weather")
            .is_none_or(|value| value == "true"),
    };
    let clock_path = "data/minecraft/world_clocks.dat";
    let mut present = false;
    if let Some(data) = saved_data(tags, clock_path)?
        && let Some(Tag::Compound(clock)) = field(data, "minecraft:overworld")
    {
        climate.day_time = match field(clock, "total_ticks") {
            Some(Tag::Long(value)) => *value,
            _ => {
                return Err(failure(
                    clock_path,
                    "invalid minecraft:overworld.total_ticks",
                ));
            }
        };
        if let Some(paused) = field(clock, "paused") {
            match paused {
                Tag::Byte(value) if matches!(value, 0 | 1) => climate.advance_time &= *value == 0,
                _ => return Err(failure(clock_path, "invalid minecraft:overworld.paused")),
            }
        }
        present = true;
    }
    let weather_path = "data/minecraft/weather.dat";
    if let Some(data) = saved_data(tags, weather_path)? {
        let boolean = |key| match field(data, key) {
            Some(Tag::Byte(value)) if matches!(value, 0 | 1) => Ok(*value != 0),
            _ => Err(failure(weather_path, format!("invalid {key}"))),
        };
        let integer = |key| match field(data, key) {
            Some(Tag::Int(value)) if *value >= 0 => Ok(i64::from(*value)),
            _ => Err(failure(weather_path, format!("invalid {key}"))),
        };
        let raining = boolean("raining")?;
        let thundering = boolean("thundering")?;
        let clear = integer("clear_weather_time")?;
        let rain = integer("rain_time")?;
        let thunder = integer("thunder_time")?;
        climate.weather_kind = if raining && thundering {
            2
        } else {
            u8::from(raining)
        };
        climate.remaining_ticks = rain;
        climate.thunder_remaining_ticks = thunder;
        climate.clear_remaining_ticks = clear;
        climate.thundering = thundering;
        climate.rain_level_bits = if raining { 1.0_f32 } else { 0.0_f32 }.to_bits();
        climate.thunder_level_bits = if thundering { 1.0_f32 } else { 0.0_f32 }.to_bits();
        present = true;
    }
    if present {
        manifest.climate = Some(climate);
    }
    let trader_path = "data/minecraft/wandering_trader.dat";
    if let Some(data) = saved_data(tags, trader_path)? {
        let integer = |key, default| match field(data, key) {
            None => Ok(default),
            Some(Tag::Int(value)) => Ok(*value),
            _ => Err(failure(trader_path, format!("invalid {key}"))),
        };
        let spawner = crate::mob_lanes::TraderSpawner {
            delay: integer("spawn_delay", 24_000)?,
            chance: integer("spawn_chance", 25)?,
        };
        manifest.wandering_trader = Some(spawner);
    }
    for (dimension, path) in BORDER_PATHS {
        let Some(data) = saved_data(tags, path)? else {
            continue;
        };
        let border = border_from_fields(data, path)?;
        if dimension == crate::dimensions::Dimension::Overworld {
            manifest.border = border;
        } else {
            manifest
                .dimensions
                .get_mut(&dimension)
                .ok_or_else(|| failure(path, "missing dimension checkpoint"))?
                .border = border;
        }
    }
    for (dimension, path) in TICKET_PATHS {
        let Some(data) = saved_data(tags, path)? else {
            continue;
        };
        let tick = manifest.tick;
        let image = if dimension == crate::dimensions::Dimension::Overworld {
            &mut *manifest
        } else {
            manifest
                .dimensions
                .get_mut(&dimension)
                .ok_or_else(|| failure(path, "missing dimension checkpoint"))?
        };
        image
            .tickets
            .retain(|owner, _| !owner.starts_with(FORCED_TICKET_OWNER));
        for entry in ticket_entries(data, path)? {
            let [x, z] = ticket_position(entry, path)?;
            let owner = format!("{FORCED_TICKET_OWNER}{x}:{z}");
            if image.tickets.insert(owner, ([x, z], (tick, 0))).is_some() {
                return Err(failure(path, format!("duplicate forced ticket at {x},{z}")));
            }
        }
    }
    Ok(())
}

/// Refresh typed mutable fields in retained templates; no filesystem or clock IO.
///
/// Unowned fields, including LevelName and LastPlayed, stay exactly as imported.
pub(crate) fn checkpoint_tags(
    tags: &mut MetadataTags,
    seed: i64,
    manifest: &CheckpointManifest,
) -> Result<(), CoreError> {
    let path = "level.dat";
    let level = root_fields_mut(tags, path)?;
    let data = compound_field_mut(level, "Data", path)?;
    let spawn = manifest
        .world_spawn
        .ok_or_else(|| failure(path, "checkpoint has no world spawn"))?;
    let [yaw, pitch] = manifest.world_spawn_rotation;
    if !yaw.is_finite() || !pitch.is_finite() {
        return Err(failure(path, "non-finite spawn rotation"));
    }
    let spawn_data = compound_field_mut(data, "spawn", path)?;
    for (key, value) in [
        ("dimension", Tag::String("minecraft:overworld".into())),
        ("pos", Tag::IntArray(spawn.to_vec())),
        ("yaw", Tag::Float((yaw + 180.0).rem_euclid(360.0) - 180.0)),
        ("pitch", Tag::Float(pitch.clamp(-90.0, 90.0))),
    ] {
        put(spawn_data, key, value);
    }
    put(
        data,
        "Time",
        Tag::Long(i64::try_from(manifest.tick).map_err(|err| failure(path, err))?),
    );
    put(
        data,
        "DataVersion",
        Tag::Int(mc_world::anvil::TARGET_DATA_VERSION),
    );
    put(data, "version", Tag::Int(19133));
    let version = compound_field_mut(data, "Version", path)?;
    for (key, value) in [
        ("Name", Tag::String("26.1.2".into())),
        ("Id", Tag::Int(mc_world::anvil::TARGET_DATA_VERSION)),
        ("Snapshot", Tag::Byte(0)),
        ("Series", Tag::String("main".into())),
    ] {
        put(version, key, value);
    }
    for (key, value) in [
        ("GameType", Tag::Int(0)),
        ("initialized", Tag::Byte(1)),
        ("allowCommands", Tag::Byte(1)),
    ] {
        if field(data, key).is_none() {
            put(data, key, value);
        }
    }
    if field(data, "difficulty_settings").is_none() {
        put(
            data,
            "difficulty_settings",
            Tag::Compound(vec![
                ("hardcore".into(), Tag::Byte(0)),
                ("locked".into(), Tag::Byte(0)),
            ]),
        );
    }
    let difficulty_settings = compound_field_mut(data, "difficulty_settings", path)?;
    let difficulty = match manifest.difficulty {
        crate::survival::Difficulty::Peaceful => "peaceful",
        crate::survival::Difficulty::Easy => "easy",
        crate::survival::Difficulty::Normal => "normal",
        crate::survival::Difficulty::Hard => "hard",
    };
    put(
        difficulty_settings,
        "difficulty",
        Tag::String(difficulty.into()),
    );
    let rules_path = "data/minecraft/game_rules.dat";
    let rules = saved_data_mut(tags, rules_path)?;
    for rule in crate::command_rules::rules() {
        let value = manifest
            .gamerules
            .get(rule.name)
            .map(String::as_str)
            .unwrap_or(rule.default);
        let tag = match rule.kind {
            crate::command_rules::RuleKind::Boolean => Tag::Byte(i8::from(
                value
                    .parse::<bool>()
                    .map_err(|err| failure(rules_path, err))?,
            )),
            crate::command_rules::RuleKind::Integer { .. } => {
                Tag::Int(value.parse().map_err(|err| failure(rules_path, err))?)
            }
        };
        put(rules, &format!("minecraft:{}", rule.name), tag);
    }
    if let Some(climate) = manifest.climate {
        let clock_path = "data/minecraft/world_clocks.dat";
        let clocks = saved_data_mut(tags, clock_path)?;
        let overworld = compound_field_mut(clocks, "minecraft:overworld", clock_path)?;
        put(overworld, "total_ticks", Tag::Long(climate.day_time));
        put(
            overworld,
            "paused",
            Tag::Byte(i8::from(!climate.advance_time)),
        );
        if field(clocks, "minecraft:the_end").is_none() {
            put(
                clocks,
                "minecraft:the_end",
                Tag::Compound(vec![(
                    "total_ticks".into(),
                    Tag::Long(
                        i64::try_from(manifest.tick).map_err(|err| failure(clock_path, err))?,
                    ),
                )]),
            );
        }
        let weather_path = "data/minecraft/weather.dat";
        let weather = saved_data_mut(tags, weather_path)?;
        let remaining =
            i32::try_from(climate.remaining_ticks).map_err(|err| failure(weather_path, err))?;
        let thunder = i32::try_from(climate.thunder_remaining_ticks)
            .map_err(|err| failure(weather_path, err))?;
        let clear = i32::try_from(climate.clear_remaining_ticks)
            .map_err(|err| failure(weather_path, err))?;
        for (key, value) in [
            ("clear_weather_time", Tag::Int(clear)),
            ("rain_time", Tag::Int(remaining)),
            ("thunder_time", Tag::Int(thunder)),
            ("raining", Tag::Byte(i8::from(climate.weather_kind != 0))),
            ("thundering", Tag::Byte(i8::from(climate.thundering))),
        ] {
            put(weather, key, value);
        }
    }
    if let Some(spawner) = manifest.wandering_trader {
        let path = "data/minecraft/wandering_trader.dat";
        let data = saved_data_mut(tags, path)?;
        put(data, "spawn_delay", Tag::Int(spawner.delay));
        put(data, "spawn_chance", Tag::Int(spawner.chance));
    }
    let worldgen_path = "data/minecraft/world_gen_settings.dat";
    if saved_data(tags, worldgen_path)?.is_none() {
        let root = root_fields_mut(tags, worldgen_path)?;
        put(
            root,
            "DataVersion",
            Tag::Int(mc_world::anvil::TARGET_DATA_VERSION),
        );
        put(root, "data", Tag::Compound(worldgen(seed)));
    }
    for (dimension, path) in BORDER_PATHS {
        let border = if dimension == crate::dimensions::Dimension::Overworld {
            manifest.border
        } else if let Some(image) = manifest.dimensions.get(&dimension) {
            image.border
        } else {
            if tags.contains_key(path) {
                return Err(failure(path, "missing dimension checkpoint"));
            }
            continue;
        };
        validate_vanilla_border(border, path)?;
        let fresh = match saved_data(tags, path)? {
            Some(data) => {
                border_from_fields(data, path)?;
                false
            }
            None => true,
        };
        let data = saved_data_mut(tags, path)?;
        for (key, value) in [
            ("center_x", Tag::Double(border.center_x)),
            ("center_z", Tag::Double(border.center_z)),
            ("size", Tag::Double(border.size)),
            ("lerp_target", Tag::Double(border.size)),
            ("lerp_time", Tag::Long(0)),
            ("damage_per_block", Tag::Double(0.2)),
            ("safe_zone", Tag::Double(5.0)),
        ] {
            put(data, key, value);
        }
        // Native metadata owns these values independently of the runtime's
        // shared geometry. Checkpoints must not normalize imported warnings.
        if fresh {
            put(data, "warning_blocks", Tag::Int(5));
            put(data, "warning_time", Tag::Int(300));
        }
    }
    for (dimension, path) in TICKET_PATHS {
        let image = if dimension == crate::dimensions::Dimension::Overworld {
            manifest
        } else if let Some(child) = manifest.dimensions.get(&dimension) {
            child
        } else if tags.contains_key(path) {
            return Err(failure(path, "missing dimension checkpoint"));
        } else {
            continue;
        };
        checkpoint_tickets(tags, path, image)?;
    }
    Ok(())
}

fn validate_vanilla_border(
    border: crate::command_border::Border,
    path: &str,
) -> Result<(), CoreError> {
    border.validate().map_err(|error| failure(path, error))?;
    if border.center_x.abs() > 29_999_984.0 || border.center_z.abs() > 29_999_984.0 {
        return Err(failure(path, "border center outside +/-29999984"));
    }
    Ok(())
}

// 26.1.2 WorldBorder$Settings.CODEC (javap), corroborated by the accepted
// vanilla-world-roundtrip/final-20261012T220527 dimension-local saved-data files.
// All nine fields are required; warning_time is ticks, not protocol seconds.
fn border_from_fields(
    data: &Fields,
    path: &str,
) -> Result<crate::command_border::Border, CoreError> {
    let required = |key| {
        let mut values = data.iter().filter(|(name, _)| name == key);
        let value = values
            .next()
            .map(|(_, value)| value)
            .ok_or_else(|| failure(path, format!("missing {key}")))?;
        if values.next().is_some() {
            return Err(failure(path, format!("duplicate {key}")));
        }
        Ok(value)
    };
    let double = |key| match required(key)? {
        Tag::Double(value) if value.is_finite() => Ok(*value),
        _ => Err(failure(path, format!("invalid double {key}"))),
    };
    let border = crate::command_border::Border {
        center_x: double("center_x")?,
        center_z: double("center_z")?,
        size: double("size")?,
    };
    validate_vanilla_border(border, path)?;
    // These semantic fields have no typed runtime owner, so retaining bytes
    // cannot make non-default settings safe to import into live gameplay.
    for (key, default) in [("damage_per_block", 0.2), ("safe_zone", 5.0)] {
        if double(key)? != default {
            return Err(failure(path, format!("unsupported non-default {key}")));
        }
    }
    for key in ["warning_blocks", "warning_time"] {
        match required(key)? {
            Tag::Int(value) if *value >= 0 => {}
            Tag::Int(_) => return Err(failure(path, format!("negative {key}"))),
            _ => return Err(failure(path, format!("invalid integer {key}"))),
        }
    }
    match required("lerp_time")? {
        Tag::Long(0) => {}
        Tag::Long(value) if *value > 0 => {
            return Err(failure(path, "unsupported timed border interpolation"));
        }
        _ => return Err(failure(path, "invalid long lerp_time")),
    }
    // A zero-time vanilla extent does not use its retained lerp target.
    double("lerp_target")?;
    Ok(border)
}

fn ticket_field<'a>(
    fields: &'a Fields,
    key: &str,
    path: &str,
) -> Result<Option<&'a Tag>, CoreError> {
    let mut values = fields.iter().filter(|(name, _)| name == key);
    let value = values.next().map(|(_, value)| value);
    if values.next().is_some() {
        return Err(failure(path, format!("duplicate ticket field {key}")));
    }
    Ok(value)
}

// TicketStorage.CODEC has an optional compound list, and Ticket.CODEC has
// required type/level plus optional ticks_left: Long defaulting to zero.
fn ticket_entries<'a>(data: &'a Fields, path: &str) -> Result<&'a [Tag], CoreError> {
    match ticket_field(data, "tickets", path)? {
        None => Ok(&[]),
        Some(Tag::List(list))
            if list.element_type == tag_type::COMPOUND
                || (list.elements.is_empty() && list.element_type == tag_type::END) =>
        {
            Ok(&list.elements)
        }
        _ => Err(failure(path, "invalid compound list tickets")),
    }
}

fn ticket_position(entry: &Tag, path: &str) -> Result<[i32; 2], CoreError> {
    let Tag::Compound(fields) = entry else {
        return Err(failure(path, "expected ticket compound"));
    };
    match ticket_field(fields, "type", path)? {
        Some(Tag::String(value)) if value == "minecraft:forced" => {}
        Some(Tag::String(value)) => {
            return Err(failure(path, format!("unsupported ticket type {value}")));
        }
        _ => return Err(failure(path, "invalid ticket type")),
    }
    match ticket_field(fields, "level", path)? {
        Some(Tag::Int(31)) => {}
        Some(Tag::Int(level)) => {
            return Err(failure(
                path,
                format!("unsupported forced ticket level {level}"),
            ));
        }
        _ => return Err(failure(path, "invalid ticket level")),
    }
    match ticket_field(fields, "ticks_left", path)? {
        None | Some(Tag::Long(0)) => {}
        Some(Tag::Long(_)) => return Err(failure(path, "unsupported forced ticket timeout")),
        _ => return Err(failure(path, "invalid ticket ticks_left")),
    }
    match ticket_field(fields, "chunk_pos", path)? {
        Some(Tag::IntArray(position)) => position
            .as_slice()
            .try_into()
            .map_err(|_| failure(path, "ticket chunk_pos requires two ints")),
        _ => Err(failure(path, "invalid ticket chunk_pos")),
    }
}

fn checkpoint_tickets(
    tags: &mut MetadataTags,
    path: &str,
    image: &CheckpointManifest,
) -> Result<(), CoreError> {
    let data = saved_data_mut(tags, path)?;
    let mut originals = BTreeMap::new();
    // Move retained compounds rather than cloning arbitrary opaque entry fields.
    let old = if let Some(index) = data.iter().position(|(name, _)| name == "tickets") {
        ticket_entries(data, path)?;
        match std::mem::replace(&mut data[index].1, Tag::Compound(Vec::new())) {
            Tag::List(list) => list.elements,
            _ => unreachable!("validated ticket list"),
        }
    } else {
        Vec::new()
    };
    for entry in old {
        let position = ticket_position(&entry, path)?;
        let Tag::Compound(fields) = entry else {
            unreachable!("validated ticket compound")
        };
        if originals.insert(position, fields).is_some() {
            return Err(failure(path, "duplicate retained forced ticket position"));
        }
    }
    let mut entries = Vec::new();
    for (owner, (position, (_, radius))) in &image.tickets {
        let Some(coordinates) = owner.strip_prefix(FORCED_TICKET_OWNER) else {
            continue;
        };
        let owner_position = coordinates
            .split_once(':')
            .and_then(|(x, z)| Some([x.parse::<i32>().ok()?, z.parse::<i32>().ok()?]));
        if owner_position != Some(*position) || *radius != 0 {
            return Err(failure(
                path,
                format!("invalid forced ticket owner/radius {owner}"),
            ));
        }
        let mut fields = originals.remove(position).unwrap_or_default();
        put(&mut fields, "chunk_pos", Tag::IntArray(position.to_vec()));
        put(&mut fields, "type", Tag::String("minecraft:forced".into()));
        put(&mut fields, "level", Tag::Int(31));
        entries.push(Tag::Compound(fields));
    }
    put(
        data,
        "tickets",
        Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements: entries,
        }),
    );
    Ok(())
}

// Vanilla's data/minecraft/worldgen/world_preset/normal.json dimension codec.
fn worldgen(seed: i64) -> Fields {
    let dimensions = [
        ("overworld", "overworld", "overworld", "overworld"),
        ("the_nether", "the_nether", "nether", "nether"),
        ("the_end", "the_end", "end", ""),
    ]
    .into_iter()
    .map(|(name, dimension_type, settings, preset)| {
        let biome = if preset.is_empty() {
            vec![("type".into(), Tag::String("minecraft:the_end".into()))]
        } else {
            vec![
                ("type".into(), Tag::String("minecraft:multi_noise".into())),
                ("preset".into(), Tag::String(format!("minecraft:{preset}"))),
            ]
        };
        (
            format!("minecraft:{name}"),
            Tag::Compound(vec![
                (
                    "type".into(),
                    Tag::String(format!("minecraft:{dimension_type}")),
                ),
                (
                    "generator".into(),
                    Tag::Compound(vec![
                        ("type".into(), Tag::String("minecraft:noise".into())),
                        (
                            "settings".into(),
                            Tag::String(format!("minecraft:{settings}")),
                        ),
                        ("biome_source".into(), Tag::Compound(biome)),
                    ]),
                ),
            ]),
        )
    })
    .collect();
    vec![
        ("seed".into(), Tag::Long(seed)),
        ("generate_structures".into(), Tag::Byte(1)),
        ("bonus_chest".into(), Tag::Byte(0)),
        ("dimensions".into(), Tag::Compound(dimensions)),
    ]
}

#[cfg(test)]
#[path = "level_metadata_trader_tests.rs"]
mod trader_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modern_metadata_checkpoint_roundtrip_uses_native_global_domains() {
        let mut tags = MetadataTags::new();
        let mut manifest = CheckpointManifest {
            tick: 79,
            difficulty: crate::survival::Difficulty::Hard,
            world_spawn: Some([12, 75, -45]),
            world_spawn_rotation: [90.0, 15.0],
            climate: Some(ClimateImage {
                day_time: 18000,
                weather_kind: 2,
                remaining_ticks: 1234,
                thunder_remaining_ticks: 4321,
                clear_remaining_ticks: 0,
                thundering: true,
                rain_level_bits: 1.0_f32.to_bits(),
                thunder_level_bits: 1.0_f32.to_bits(),
                advance_time: false,
                advance_weather: true,
            }),
            ..Default::default()
        };
        manifest
            .gamerules
            .insert("advance_time".into(), "false".into());
        manifest
            .gamerules
            .insert("random_tick_speed".into(), "7".into());
        checkpoint_tags(&mut tags, 42, &manifest).unwrap();
        let level = root_fields(&tags, "level.dat").unwrap().unwrap();
        let Some(Tag::Compound(data)) = field(level, "Data") else {
            panic!("missing Data")
        };
        assert_eq!(field(data, "Time"), Some(&Tag::Long(79)));
        assert_eq!(field(data, "version"), Some(&Tag::Int(19133)));
        assert_eq!(field(data, "DataVersion"), Some(&Tag::Int(4790)));
        assert!(matches!(field(data, "spawn"), Some(Tag::Compound(_))));
        for obsolete in [
            "SpawnX",
            "SpawnY",
            "SpawnZ",
            "SpawnAngle",
            "DayTime",
            "raining",
            "GameRules",
            "WorldGenSettings",
        ] {
            assert!(field(data, obsolete).is_none(), "{obsolete}");
        }
        let rules = saved_data(&tags, "data/minecraft/game_rules.dat")
            .unwrap()
            .unwrap();
        assert_eq!(field(rules, "minecraft:advance_time"), Some(&Tag::Byte(0)));
        assert_eq!(
            field(rules, "minecraft:random_tick_speed"),
            Some(&Tag::Int(7))
        );
        let worldgen = saved_data(&tags, "data/minecraft/world_gen_settings.dat")
            .unwrap()
            .unwrap();
        assert_eq!(field(worldgen, "seed"), Some(&Tag::Long(42)));
        let Some(Tag::Compound(dimensions)) = field(worldgen, "dimensions") else {
            panic!("missing dimensions")
        };
        assert_eq!(dimensions.len(), 3);
        let mut imported = CheckpointManifest::default();
        import_tags(&tags, &mut imported).unwrap();
        assert_eq!(imported.tick, 79);
        assert_eq!(imported.difficulty, crate::survival::Difficulty::Hard);
        assert_eq!(imported.world_spawn, manifest.world_spawn);
        assert_eq!(imported.world_spawn_rotation, manifest.world_spawn_rotation);
        assert_eq!(imported.climate, manifest.climate);
        assert_eq!(imported.gamerules["random_tick_speed"], "7");
        assert_eq!(imported.gamerules["advance_time"], "false");
        // Imported fields outside v2 ownership survive derived-artifact refreshes.
        let level = tags.get_mut("level.dat").unwrap();
        let Tag::Compound(level) = level else {
            unreachable!()
        };
        put(level, "OpaqueRoot", Tag::Long(37));
        let Some((_, Tag::Compound(data))) = level.iter_mut().find(|(key, _)| key == "Data") else {
            unreachable!()
        };
        data.push(("Unowned".into(), Tag::String("retained".into())));
        data.push(("LastPlayed".into(), Tag::Long(123)));
        for key in ["spawn", "Version", "difficulty_settings"] {
            let fields = compound_field_mut(data, key, "level.dat").unwrap();
            put(fields, "OpaqueNested", Tag::Long(37));
        }
        for path in [
            "data/minecraft/game_rules.dat",
            "data/minecraft/world_clocks.dat",
            "data/minecraft/weather.dat",
            "data/minecraft/world_gen_settings.dat",
        ] {
            let root = root_fields_mut(&mut tags, path).unwrap();
            put(root, "OpaqueRoot", Tag::Long(37));
            let data = compound_field_mut(root, "data", path).unwrap();
            put(data, "OpaqueData", Tag::Long(37));
        }
        let clocks = saved_data_mut(&mut tags, "data/minecraft/world_clocks.dat").unwrap();
        let clock = compound_field_mut(clocks, "minecraft:overworld", "world_clocks").unwrap();
        put(clock, "OpaqueClock", Tag::Long(37));
        manifest.tick = 80;
        checkpoint_tags(&mut tags, 42, &manifest).unwrap();
        let level = root_fields(&tags, "level.dat").unwrap().unwrap();
        let Some(Tag::Compound(data)) = field(level, "Data") else {
            unreachable!()
        };
        assert_eq!(
            field(data, "Unowned"),
            Some(&Tag::String("retained".into()))
        );
        assert_eq!(field(data, "Time"), Some(&Tag::Long(80)));
        assert_eq!(field(data, "LastPlayed"), Some(&Tag::Long(123)));
        assert_eq!(field(level, "OpaqueRoot"), Some(&Tag::Long(37)));
        for key in ["spawn", "Version", "difficulty_settings"] {
            let Some(Tag::Compound(fields)) = field(data, key) else {
                panic!("missing {key}")
            };
            assert_eq!(field(fields, "OpaqueNested"), Some(&Tag::Long(37)));
        }
        for path in [
            "data/minecraft/game_rules.dat",
            "data/minecraft/world_clocks.dat",
            "data/minecraft/weather.dat",
            "data/minecraft/world_gen_settings.dat",
        ] {
            let root = root_fields(&tags, path).unwrap().unwrap();
            let data = saved_data(&tags, path).unwrap().unwrap();
            assert_eq!(field(root, "OpaqueRoot"), Some(&Tag::Long(37)), "{path}");
            assert_eq!(field(data, "OpaqueData"), Some(&Tag::Long(37)), "{path}");
        }
        let clocks = saved_data(&tags, "data/minecraft/world_clocks.dat")
            .unwrap()
            .unwrap();
        let Some(Tag::Compound(clock)) = field(clocks, "minecraft:overworld") else {
            panic!("missing overworld clock")
        };
        assert_eq!(field(clock, "OpaqueClock"), Some(&Tag::Long(37)));
        assert_eq!(from_tag(&to_tag(&tags)).unwrap(), tags);
    }

    #[test]
    fn old_level_spawn_fails_without_modern_output() {
        let tags = [(
            "level.dat".into(),
            Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![
                    ("SpawnX".into(), Tag::Int(1)),
                    ("SpawnY".into(), Tag::Int(64)),
                    ("SpawnZ".into(), Tag::Int(2)),
                ]),
            )]),
        )]
        .into_iter()
        .collect();
        let error = import_tags(&tags, &mut CheckpointManifest::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("Data.spawn"), "{error}");
        assert!(error.contains("legacy SpawnX"), "{error}");
    }

    #[test]
    fn template_envelope_rejects_unsafe_duplicate_and_noncompound_records() {
        for path in [
            "",
            "/level.dat",
            "../level.dat",
            "data/../level.dat",
            "./level.dat",
            "data//level.dat",
            "C:/level.dat",
            "data\\level.dat",
            "data/\0level.dat",
        ] {
            let tag = Tag::Compound(vec![(path.into(), Tag::Compound(vec![]))]);
            assert!(from_tag(&tag).is_err(), "{path:?}");
        }
        let duplicate = Tag::Compound(vec![
            ("level.dat".into(), Tag::Compound(vec![])),
            ("level.dat".into(), Tag::Compound(vec![])),
        ]);
        assert!(from_tag(&duplicate).is_err());
        assert!(from_tag(&Tag::Compound(vec![("level.dat".into(), Tag::Byte(0))])).is_err());
        assert!(from_tag(&Tag::Byte(0)).is_err());
    }

    #[test]
    fn dimension_borders_roundtrip_and_preserve_unknown_fields() {
        let mut manifest = CheckpointManifest {
            world_spawn: Some([0, 80, 0]),
            border: crate::command_border::Border {
                center_x: 248.5,
                center_z: -184.5,
                size: 128_000.0,
            },
            ..Default::default()
        };
        for (dimension, _) in BORDER_PATHS.iter().skip(1) {
            let mut border = manifest.border;
            if *dimension == crate::dimensions::Dimension::Nether {
                border.center_x = 31.0625;
                border.center_z = -23.0625;
            }
            manifest.dimensions.insert(
                *dimension,
                Box::new(CheckpointManifest {
                    border,
                    ..Default::default()
                }),
            );
        }
        let mut tags = MetadataTags::new();
        checkpoint_tags(&mut tags, 42, &manifest).unwrap();
        for (_, path) in BORDER_PATHS {
            let root = root_fields_mut(&mut tags, path).unwrap();
            put(root, "OpaqueRoot", Tag::Long(37));
            let data = compound_field_mut(root, "data", path).unwrap();
            put(data, "OpaqueData", Tag::Long(37));
            put(data, "warning_time", Tag::Int(15));
            assert_eq!(field(data, "lerp_time"), Some(&Tag::Long(0)));
        }
        let mut imported = CheckpointManifest::default();
        for (dimension, _) in BORDER_PATHS.iter().skip(1) {
            imported.dimensions.insert(*dimension, Box::default());
        }
        import_tags(&tags, &mut imported).unwrap();
        assert_eq!(imported.border, manifest.border);
        for (&dimension, image) in &manifest.dimensions {
            assert_eq!(imported.dimensions[&dimension].border, image.border);
        }
        imported
            .dimensions
            .get_mut(&crate::dimensions::Dimension::Nether)
            .unwrap()
            .border
            .size = 321.0;
        checkpoint_tags(&mut tags, 42, &imported).unwrap();
        for (dimension, path) in BORDER_PATHS {
            let root = root_fields(&tags, path).unwrap().unwrap();
            let data = saved_data(&tags, path).unwrap().unwrap();
            let border = if dimension == crate::dimensions::Dimension::Overworld {
                imported.border
            } else {
                imported.dimensions[&dimension].border
            };
            assert_eq!(field(root, "OpaqueRoot"), Some(&Tag::Long(37)));
            assert_eq!(field(data, "OpaqueData"), Some(&Tag::Long(37)));
            assert_eq!(field(data, "center_x"), Some(&Tag::Double(border.center_x)));
            assert_eq!(field(data, "center_z"), Some(&Tag::Double(border.center_z)));
            assert_eq!(field(data, "warning_time"), Some(&Tag::Int(15)));
            assert_eq!(field(data, "size"), Some(&Tag::Double(border.size)));
            assert_eq!(field(data, "lerp_target"), Some(&Tag::Double(border.size)));
        }
    }

    #[test]
    fn native_warning_metadata_is_lossless_across_geometry_updates() {
        let mut manifest = CheckpointManifest {
            world_spawn: Some([0, 80, 0]),
            ..Default::default()
        };
        for (dimension, _) in BORDER_PATHS.iter().skip(1) {
            manifest.dimensions.insert(*dimension, Box::default());
        }
        let mut tags = MetadataTags::new();
        checkpoint_tags(&mut tags, 42, &manifest).unwrap();
        let warnings = [(0, 15), (10, 0), (i32::MAX, i32::MAX)];
        for ((_, path), (blocks, time)) in BORDER_PATHS.into_iter().zip(warnings) {
            let data = saved_data_mut(&mut tags, path).unwrap();
            put(data, "warning_blocks", Tag::Int(blocks));
            put(data, "warning_time", Tag::Int(time));
        }
        let mut recovered = from_tag(&to_tag(&tags)).unwrap();
        import_tags(&recovered, &mut manifest).unwrap();
        manifest.border.size = 321.0;
        checkpoint_tags(&mut recovered, 42, &manifest).unwrap();
        for ((dimension, path), (blocks, time)) in BORDER_PATHS.into_iter().zip(warnings) {
            let data = saved_data(&recovered, path).unwrap().unwrap();
            let border = if dimension == crate::dimensions::Dimension::Overworld {
                manifest.border
            } else {
                manifest.dimensions[&dimension].border
            };
            assert_eq!(field(data, "size"), Some(&Tag::Double(border.size)));
            assert_eq!(field(data, "warning_blocks"), Some(&Tag::Int(blocks)));
            assert_eq!(field(data, "warning_time"), Some(&Tag::Int(time)));
        }
    }

    #[test]
    fn malformed_or_unsupported_world_border_semantics_fail_closed() {
        let manifest = CheckpointManifest {
            world_spawn: Some([0, 80, 0]),
            ..Default::default()
        };
        let mut valid = MetadataTags::new();
        checkpoint_tags(&mut valid, 42, &manifest).unwrap();
        let path = BORDER_PATHS[0].1;
        for (key, value) in [
            ("center_x", Tag::Double(f64::NAN)),
            ("center_z", Tag::Double(30_000_000.0)),
            ("size", Tag::Double(0.0)),
            ("size", Tag::Int(100)),
            ("damage_per_block", Tag::Double(0.3)),
            ("safe_zone", Tag::Double(10.0)),
            ("warning_blocks", Tag::Int(-1)),
            ("warning_time", Tag::Int(-1)),
            ("warning_blocks", Tag::Long(5)),
            ("warning_time", Tag::Float(15.0)),
            ("lerp_time", Tag::Long(20)),
            ("lerp_time", Tag::Long(-1)),
            ("lerp_time", Tag::Int(0)),
            ("lerp_target", Tag::Double(f64::INFINITY)),
        ] {
            let mut tags = valid.clone();
            put(saved_data_mut(&mut tags, path).unwrap(), key, value);
            let error = import_tags(&tags, &mut CheckpointManifest::default())
                .unwrap_err()
                .to_string();
            assert!(error.contains(path), "{error}");
            assert!(checkpoint_tags(&mut tags, 42, &manifest).is_err(), "{key}");
        }
        let mut missing = valid.clone();
        saved_data_mut(&mut missing, path)
            .unwrap()
            .retain(|(key, _)| key != "warning_time");
        assert!(import_tags(&missing, &mut CheckpointManifest::default()).is_err());
        let mut duplicate = valid;
        saved_data_mut(&mut duplicate, path)
            .unwrap()
            .push(("size".into(), Tag::Double(100.0)));
        assert!(import_tags(&duplicate, &mut CheckpointManifest::default()).is_err());
    }

    #[test]
    fn forced_tickets_import_all_dimensions_and_removals_do_not_resurrect() {
        let mut manifest = CheckpointManifest {
            tick: 79,
            world_spawn: Some([0, 80, 0]),
            ..Default::default()
        };
        for (dimension, _) in TICKET_PATHS.iter().skip(1) {
            manifest.dimensions.insert(*dimension, Box::default());
        }
        for (index, (dimension, _)) in TICKET_PATHS.iter().enumerate() {
            let image = if *dimension == crate::dimensions::Dimension::Overworld {
                &mut manifest
            } else {
                manifest.dimensions.get_mut(dimension).unwrap()
            };
            let x = index as i32;
            image.tickets.insert(
                format!("{FORCED_TICKET_OWNER}{x}:{}", -x),
                ([x, -x], (79, 0)),
            );
        }
        let mut tags = MetadataTags::new();
        checkpoint_tags(&mut tags, 42, &manifest).unwrap();
        for (_, path) in TICKET_PATHS {
            let root = root_fields_mut(&mut tags, path).unwrap();
            put(root, "OpaqueRoot", Tag::Long(37));
            let data = compound_field_mut(root, "data", path).unwrap();
            put(data, "OpaqueData", Tag::Long(37));
            let Some((_, Tag::List(list))) = data.iter_mut().find(|(key, _)| key == "tickets")
            else {
                panic!("missing tickets")
            };
            let Tag::Compound(entry) = &mut list.elements[0] else {
                panic!("missing ticket entry")
            };
            put(entry, "OpaqueEntry", Tag::Long(37));
        }
        let mut imported = CheckpointManifest::default();
        for (dimension, _) in TICKET_PATHS.iter().skip(1) {
            imported.dimensions.insert(*dimension, Box::default());
        }
        import_tags(&tags, &mut imported).unwrap();
        assert_eq!(imported.tick, 79);
        assert_eq!(imported.tickets, manifest.tickets);
        for (dimension, _) in TICKET_PATHS.iter().skip(1) {
            assert_eq!(
                imported.dimensions[dimension].tickets,
                manifest.dimensions[dimension].tickets
            );
        }
        imported.tickets.clear();
        checkpoint_tags(&mut tags, 42, &imported).unwrap();
        for (dimension, path) in TICKET_PATHS {
            let root = root_fields(&tags, path).unwrap().unwrap();
            let data = saved_data(&tags, path).unwrap().unwrap();
            assert_eq!(field(root, "OpaqueRoot"), Some(&Tag::Long(37)));
            assert_eq!(field(data, "OpaqueData"), Some(&Tag::Long(37)));
            let entries = ticket_entries(data, path).unwrap();
            if dimension == crate::dimensions::Dimension::Overworld {
                assert!(entries.is_empty());
            } else {
                assert_eq!(entries.len(), 1);
                let Tag::Compound(entry) = &entries[0] else {
                    panic!("missing ticket entry")
                };
                assert_eq!(field(entry, "OpaqueEntry"), Some(&Tag::Long(37)));
            }
        }
        import_tags(&tags, &mut imported).unwrap();
        assert!(imported.tickets.is_empty());
    }

    #[test]
    fn malformed_or_unsupported_forced_ticket_records_fail_closed() {
        let path = TICKET_PATHS[0].1;
        let valid = vec![
            ("chunk_pos".into(), Tag::IntArray(vec![0, 0])),
            ("type".into(), Tag::String("minecraft:forced".into())),
            ("level".into(), Tag::Int(31)),
        ];
        assert_eq!(
            ticket_position(&Tag::Compound(valid.clone()), path).unwrap(),
            [0, 0]
        );
        for (key, value) in [
            ("chunk_pos", Tag::IntArray(vec![0])),
            ("chunk_pos", Tag::LongArray(vec![0, 0])),
            ("type", Tag::String("minecraft:player".into())),
            ("type", Tag::Int(0)),
            ("level", Tag::Int(32)),
            ("level", Tag::Byte(31)),
            ("ticks_left", Tag::Long(1)),
            ("ticks_left", Tag::Long(-1)),
            ("ticks_left", Tag::Int(0)),
        ] {
            let mut fields = valid.clone();
            put(&mut fields, key, value);
            assert!(
                ticket_position(&Tag::Compound(fields), path).is_err(),
                "{key}"
            );
        }
        for key in ["type", "level", "chunk_pos"] {
            let mut fields = valid.clone();
            fields.retain(|(name, _)| name != key);
            assert!(
                ticket_position(&Tag::Compound(fields), path).is_err(),
                "{key}"
            );
        }
        let mut explicit_zero = valid.clone();
        put(&mut explicit_zero, "ticks_left", Tag::Long(0));
        assert!(ticket_position(&Tag::Compound(explicit_zero), path).is_ok());
        let mut duplicate = valid.clone();
        duplicate.push(("level".into(), Tag::Int(31)));
        assert!(ticket_position(&Tag::Compound(duplicate), path).is_err());
        let mut tags = MetadataTags::new();
        put(
            saved_data_mut(&mut tags, path).unwrap(),
            "tickets",
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: vec![Tag::Compound(valid.clone()), Tag::Compound(valid)],
            }),
        );
        assert!(import_tags(&tags, &mut CheckpointManifest::default()).is_err());
    }
}
