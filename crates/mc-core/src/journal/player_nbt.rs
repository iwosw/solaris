//! Player image <-> vanilla NBT conversion and active-effect codecs.
//! Moved verbatim from `journal.rs`.

use super::*;

/// Encode a complete player image without touching filesystem state.
pub(crate) fn player_to_tag(
    record: &ReplayPlayer,
    items: &mc_data::items::ItemRegistry,
) -> Result<mc_nbt::Tag, CoreError> {
    player_image_to_tag(record, items, true)
}

/// Vanilla command image, without private custody, statistics or advancement documents.
pub(crate) fn player_query_tag(
    record: &ReplayPlayer,
    items: &mc_data::items::ItemRegistry,
) -> Result<mc_nbt::Tag, CoreError> {
    player_image_to_tag(record, items, false)
}

pub(super) fn player_image_to_tag(
    record: &ReplayPlayer,
    items: &mc_data::items::ItemRegistry,
    persistent: bool,
) -> Result<mc_nbt::Tag, CoreError> {
    use mc_data::player_nbt::{VanillaPlayer, player_to_fields};
    crate::ender_chest::contents(&record.opaque_player_fields, items)?;
    let mut opaque_fields = if record.opaque_player_fields.is_empty() {
        Vec::new()
    } else {
        let mut cursor = std::io::Cursor::new(&record.opaque_player_fields);
        let (_, tag) = mc_nbt::read_named(&mut cursor)
            .map_err(|err| CoreError::Storage(format!("player opaque fields: {err}")))?;
        if cursor.position() != record.opaque_player_fields.len() as u64 {
            return Err(CoreError::Storage(
                "player opaque fields: trailing NBT bytes".into(),
            ));
        }
        let mc_nbt::Tag::Compound(fields) = tag else {
            return Err(CoreError::Storage(
                "player opaque fields: expected compound".into(),
            ));
        };
        fields
    };
    // Exclude every owned key, even when its fresh optional value is absent.
    opaque_fields.retain(|(key, _)| !player_owned_field(key));
    let mut inventory = Vec::new();
    let mut equipment = Vec::new();
    for (index, stack) in record.inventory.iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        if let Some(slot) = v2_slot_to_storage(index) {
            inventory.push((slot, stack.clone()));
        } else if let Some(name) = v2_slot_to_equipment(index) {
            equipment.push((name.to_owned(), stack.clone()));
        }
    }
    let player = VanillaPlayer {
        pos: record.pos,
        yaw: record.yaw,
        pitch: record.pitch,
        health: record.health,
        food: record.food,
        saturation: record.saturation,
        exhaustion: record.exhaustion,
        air: record.air,
        selected: record.selected,
        xp_level: record.experience_level,
        xp_progress: record.experience_progress,
        xp_total: record.experience_total,
        game_mode: i32::from(record.game_mode),
        inventory,
        equipment,
        carried: record.carried.clone(),
        grid: record.grid.clone(),
    };
    let mut fields =
        player_to_fields(items, &player).map_err(|err| CoreError::Storage(err.to_string()))?;
    fields.push((
        "Dimension".into(),
        mc_nbt::Tag::String(record.dimension.name().into()),
    ));
    fields.extend(active_effects_to_fields(&record.active_effects));
    fields.extend(crate::portals::fields(record.portal));
    fields.push((
        "XpSeed".into(),
        mc_nbt::Tag::Int(record.enchantment_offer_seed),
    ));
    fields.push((
        "SolarisGameplaySeed".into(),
        mc_nbt::Tag::Int(record.enchantment_seed),
    ));
    let mut warning = record.warden_warning;
    warning.advance_tick(record.tick);
    fields.push(("warden_spawn_tracker".into(), warning.nbt()));
    if persistent {
        fields.push((
            "SolarisWardenWarning".into(),
            mc_nbt::Tag::ByteArray(
                serde_json::to_vec(&warning)
                    .map_err(|err| CoreError::Storage(format!("player warning: {err}")))?
                    .into_iter()
                    .map(|byte| byte as i8)
                    .collect(),
            ),
        ));
    }
    if let Some(pos) = record.current_impulse_impact_pos {
        fields.push((
            "current_explosion_impact_pos".into(),
            mc_nbt::Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::DOUBLE,
                elements: pos.into_iter().map(mc_nbt::Tag::Double).collect(),
            }),
        ));
    }
    fields.push((
        "FallFlying".into(),
        mc_nbt::Tag::Byte(i8::from(record.fall_flying)),
    ));
    fields.push((
        "Fire".into(),
        mc_nbt::Tag::Short(i16::try_from(record.burn_ticks).unwrap_or(i16::MAX)),
    ));
    // Vanilla `DataVersion` (26.1.2 world version, shared with chunk/entity
    // writers) so vanilla runs the right DataFixer chain on our files.
    fields.push((
        "DataVersion".into(),
        mc_nbt::Tag::Int(mc_world::anvil::TARGET_DATA_VERSION),
    ));
    if persistent {
        fields.push((
            "SolarisStatistics".into(),
            mc_nbt::Tag::ByteArray(
                serde_json::to_vec(&record.statistics)
                    .map_err(|err| CoreError::Storage(format!("player statistics: {err}")))?
                    .into_iter()
                    .map(|byte| byte as i8)
                    .collect(),
            ),
        ));
        // Solaris custody keys (vanilla ignores unknown keys).
        for (key, value) in solaris_custody_fields(items, &player)
            .map_err(|err| CoreError::Storage(err.to_string()))?
        {
            fields.push((key, value));
        }
    }
    if let Some(position) = record.respawn {
        fields.push((
            "respawn".into(),
            mc_nbt::Tag::Compound(vec![
                (
                    "dimension".into(),
                    mc_nbt::Tag::String(record.respawn_dimension.name().into()),
                ),
                ("pos".into(), mc_nbt::Tag::IntArray(position.to_vec())),
                ("yaw".into(), mc_nbt::Tag::Float(record.respawn_yaw)),
                ("pitch".into(), mc_nbt::Tag::Float(record.respawn_pitch)),
                (
                    "forced".into(),
                    mc_nbt::Tag::Byte(i8::from(record.respawn_forced)),
                ),
            ]),
        ));
    }
    if persistent && record.respawn.is_none() {
        fields.push((
            "solaris:RespawnState".into(),
            mc_nbt::Tag::Compound(vec![
                (
                    "dimension".into(),
                    mc_nbt::Tag::String(record.respawn_dimension.name().into()),
                ),
                ("yaw".into(), mc_nbt::Tag::Float(record.respawn_yaw)),
                ("pitch".into(), mc_nbt::Tag::Float(record.respawn_pitch)),
                (
                    "forced".into(),
                    mc_nbt::Tag::Byte(i8::from(record.respawn_forced)),
                ),
            ]),
        ));
    }
    if persistent {
        fields.push(("solaris:Tick".into(), mc_nbt::Tag::Long(record.tick as i64)));
        fields.push((
            "solaris:BurnTicks".into(),
            mc_nbt::Tag::Long(i64::from(record.burn_ticks)),
        ));
        fields.push((
            "SolarisAdvancements".into(),
            // Vanilla's complete document can exceed NBT's 64 KiB string limit.
            mc_nbt::Tag::ByteArray(
                serde_json::to_vec(&record.advancement_progress)
                    .map_err(|err| CoreError::Storage(format!("player advancements: {err}")))?
                    .into_iter()
                    .map(|byte| byte as i8)
                    .collect(),
            ),
        ));
        fields.push((
            "SolarisUnlockedRecipes".into(),
            mc_nbt::Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::STRING,
                elements: record
                    .unlocked_recipes
                    .iter()
                    .cloned()
                    .map(mc_nbt::Tag::String)
                    .collect(),
            }),
        ));
    }
    fields.extend(opaque_fields);
    Ok(mc_nbt::Tag::Compound(fields))
}

/// v2 inventory index -> vanilla storage slot (0-35). Armor (36-39) and
/// offhand (40) ride the vanilla `equipment` compound, never `Inventory`.
pub(super) fn v2_slot_to_storage(index: usize) -> Option<u8> {
    match index {
        0..=35 => Some(index as u8),
        _ => None,
    }
}

/// Vanilla storage slot (0-35) -> v2 inventory index.
pub(super) fn storage_slot_to_v2(slot: u8) -> Option<usize> {
    match slot {
        0..=35 => Some(usize::from(slot)),
        _ => None,
    }
}

/// v2 armor/offhand index -> vanilla `equipment` slot name.
pub(super) fn v2_slot_to_equipment(index: usize) -> Option<&'static str> {
    match index {
        36 => Some("feet"),
        37 => Some("legs"),
        38 => Some("chest"),
        39 => Some("head"),
        40 => Some("offhand"),
        _ => None,
    }
}

/// Vanilla `equipment` slot name -> v2 inventory index.
pub(super) fn equipment_to_v2_slot(name: &str) -> Option<usize> {
    match name {
        "feet" => Some(36),
        "legs" => Some(37),
        "chest" => Some(38),
        "head" => Some(39),
        "offhand" => Some(40),
        _ => None,
    }
}

/// Solaris custody keys: cursor + crafting grid as vanilla item compounds.
pub(super) fn solaris_custody_fields(
    items: &mc_data::items::ItemRegistry,
    player: &mc_data::player_nbt::VanillaPlayer,
) -> Result<Vec<(String, mc_nbt::Tag)>, mc_data::item_nbt::ItemNbtError> {
    use mc_data::item_nbt::stack_to_fields;
    let mut out = Vec::new();
    if !player.carried.is_empty() {
        out.push((
            "solaris:Carried".to_owned(),
            mc_nbt::Tag::Compound(stack_to_fields(items, &player.carried)?),
        ));
    }
    let grid: Vec<mc_nbt::Tag> = player
        .grid
        .iter()
        // Empty compounds retain cell positions through the shared item codec.
        .map(|stack| {
            if stack.is_empty() {
                Ok(mc_nbt::Tag::Compound(Vec::new()))
            } else {
                stack_to_fields(items, stack).map(mc_nbt::Tag::Compound)
            }
        })
        .collect::<Result<_, _>>()?;
    if !grid.is_empty() {
        out.push((
            "solaris:CraftingGrid".to_owned(),
            mc_nbt::Tag::List(mc_nbt::ListTag {
                element_type: if grid.is_empty() {
                    mc_nbt::tag_type::END
                } else {
                    mc_nbt::tag_type::COMPOUND
                },
                elements: grid,
            }),
        ));
    }
    Ok(out)
}

/// Decode a player image without touching filesystem state.
pub(crate) fn player_from_tag(
    root: &mc_nbt::Tag,
    items: &mc_data::items::ItemRegistry,
) -> Result<ReplayPlayer, CoreError> {
    use mc_data::player_nbt::{VanillaPlayer, player_from_fields};
    let failure =
        |stage: &str, error: String| CoreError::Storage(format!("player record: {stage}: {error}"));
    let mc_nbt::Tag::Compound(entries) = root else {
        return Err(failure("NBT", "root is not a compound".into()));
    };
    let player = player_from_fields(items, VanillaPlayer::default(), entries)
        .map_err(|err| failure("player fields", err.to_string()))?;
    let mut inventory = vec![mc_data::ItemStack::EMPTY; 41];
    for (slot, stack) in player.inventory {
        if let Some(index) = storage_slot_to_v2(slot) {
            inventory[index] = stack;
        }
    }
    for (name, stack) in player.equipment {
        if let Some(index) = equipment_to_v2_slot(&name) {
            inventory[index] = stack;
        }
    }
    let find = |name: &str| {
        entries
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, tag)| tag)
    };
    let advancement_progress = match find("SolarisAdvancements") {
        Some(mc_nbt::Tag::ByteArray(json)) => {
            let bytes: Vec<_> = json.iter().map(|byte| *byte as u8).collect();
            Arc::new(
                serde_json::from_slice(&bytes)
                    .map_err(|err| failure("SolarisAdvancements", err.to_string()))?,
            )
        }
        None => empty_advancement_progress(),
        Some(_) => {
            return Err(failure(
                "SolarisAdvancements",
                "expected JSON byte array".into(),
            ));
        }
    };
    // Native custody wins; vanilla imports project only the owned recipe set.
    // The rest of recipeBook remains in opaque_player_fields.
    let recipe_list = match find("SolarisUnlockedRecipes") {
        Some(tag) => Some(tag),
        None => match find("recipeBook") {
            Some(mc_nbt::Tag::Compound(book)) => book
                .iter()
                .find(|(key, _)| key == "recipes")
                .map(|(_, tag)| tag),
            None => None,
            Some(_) => return Err(failure("recipeBook", "expected compound".into())),
        },
    };
    let unlocked_recipes = Arc::new(match recipe_list {
        Some(mc_nbt::Tag::List(list))
            if list.element_type == mc_nbt::tag_type::STRING || list.elements.is_empty() =>
        {
            list.elements
                .iter()
                .map(|tag| match tag {
                    mc_nbt::Tag::String(recipe) => Ok(recipe.clone()),
                    _ => Err(failure("unlocked recipes", "expected string list".into())),
                })
                .collect::<Result<BTreeSet<_>, _>>()?
        }
        None => BTreeSet::new(),
        Some(_) => return Err(failure("unlocked recipes", "expected string list".into())),
    });
    let carried = match find("solaris:Carried") {
        Some(mc_nbt::Tag::Compound(fields)) => mc_data::item_nbt::stack_from_fields(items, fields)
            .map_err(|err| failure("solaris:Carried slot 0", err.to_string()))?
            .unwrap_or(mc_data::ItemStack::EMPTY),
        Some(_) => {
            return Err(failure(
                "solaris:Carried slot 0",
                "expected item compound".into(),
            ));
        }
        None => mc_data::ItemStack::EMPTY,
    };
    let grid = match find("solaris:CraftingGrid") {
        Some(mc_nbt::Tag::List(list)) => {
            let mut grid = Vec::with_capacity(list.elements.len());
            for (slot, element) in list.elements.iter().enumerate() {
                let mc_nbt::Tag::Compound(fields) = element else {
                    return Err(failure(
                        &format!("solaris:CraftingGrid slot {slot}"),
                        "expected item compound".into(),
                    ));
                };
                let stack = mc_data::item_nbt::stack_from_fields(items, fields).map_err(|err| {
                    failure(
                        &format!("solaris:CraftingGrid slot {slot}"),
                        err.to_string(),
                    )
                })?;
                grid.push(stack.unwrap_or(mc_data::ItemStack::EMPTY));
            }
            grid
        }
        Some(_) => return Err(failure("solaris:CraftingGrid", "expected item list".into())),
        None => Vec::new(),
    };
    let respawn_fields = match find("respawn").or_else(|| find("solaris:RespawnState")) {
        Some(mc_nbt::Tag::Compound(fields)) => Some(fields),
        Some(_) => return Err(failure("respawn", "expected compound".into())),
        None if [
            "solaris:Respawn",
            "SpawnX",
            "SpawnY",
            "SpawnZ",
            "SpawnDimension",
            "SpawnForced",
            "SpawnAngle",
            "SpawnPitch",
        ]
        .iter()
        .any(|key| find(key).is_some()) =>
        {
            return Err(failure("respawn", "unsupported legacy respawn keys; Minecraft 26.1 requires respawn compound (no legacy fallback)".into()));
        }
        None => None,
    };
    let respawn_field = |key| {
        respawn_fields.and_then(|fields| {
            fields
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value)
        })
    };
    let respawn = match respawn_field("pos") {
        Some(mc_nbt::Tag::IntArray(cell)) if cell.len() == 3 => Some([cell[0], cell[1], cell[2]]),
        None if respawn_fields.is_none() || find("solaris:RespawnState").is_some() => None,
        _ => return Err(failure("respawn.pos", "expected three integers".into())),
    };
    let respawn_dimension = match respawn_field("dimension") {
        None if respawn_fields.is_none() => crate::dimensions::Dimension::Overworld,
        Some(mc_nbt::Tag::String(name)) => crate::dimensions::Dimension::parse(name)
            .filter(|dimension| dimension.name() == name)
            .ok_or_else(|| failure("respawn.dimension", format!("unknown dimension {name}")))?,
        _ => return Err(failure("respawn.dimension", "expected string".into())),
    };
    let respawn_forced = match respawn_field("forced") {
        None => false,
        Some(mc_nbt::Tag::Byte(value)) if matches!(value, 0 | 1) => *value != 0,
        _ => return Err(failure("respawn.forced", "expected boolean".into())),
    };
    let spawn_angle = |key, bound: f32| -> Result<f32, CoreError> {
        match respawn_field(key) {
            None if respawn_fields.is_none() => Ok(0.0),
            Some(mc_nbt::Tag::Float(value))
                if value.is_finite() && (-bound..=bound).contains(value) =>
            {
                Ok(*value)
            }
            _ => Err(failure(
                &format!("respawn.{key}"),
                format!("expected finite float in -{bound}..={bound}"),
            )),
        }
    };
    let opaque_fields: Vec<_> = entries
        .iter()
        .filter(|(key, _)| !player_owned_field(key))
        .cloned()
        .map(|(key, mut tag)| {
            if key == "recipeBook"
                && let mc_nbt::Tag::Compound(book) = &mut tag
            {
                book.retain(|(name, _)| name != "recipes");
            }
            (key, tag)
        })
        .collect();
    let mut opaque_player_fields = Vec::new();
    if !opaque_fields.is_empty() {
        mc_nbt::write_named(
            &mut opaque_player_fields,
            "",
            &mc_nbt::Tag::Compound(opaque_fields),
        )
        .map_err(|err| failure("opaque fields", err.to_string()))?;
    }
    crate::ender_chest::contents(&opaque_player_fields, items)?;
    let mut record = ReplayPlayer {
        opaque_player_fields,
        statistics: match find("SolarisStatistics") {
            None => mc_data::statistics::PlayerStatistics::default(),
            Some(mc_nbt::Tag::ByteArray(bytes)) => {
                let bytes: Vec<_> = bytes.iter().map(|byte| *byte as u8).collect();
                serde_json::from_slice(&bytes)
                    .map_err(|err| failure("SolarisStatistics", err.to_string()))?
            }
            Some(_) => return Err(failure("SolarisStatistics", "expected byte array".into())),
        },
        advancement_progress,
        unlocked_recipes,
        dimension: match find("Dimension") {
            None => crate::dimensions::Dimension::Overworld,
            Some(mc_nbt::Tag::String(name)) => crate::dimensions::Dimension::parse(name)
                .filter(|dimension| dimension.name() == name)
                .ok_or_else(|| failure("Dimension", format!("unknown dimension {name}")))?,
            Some(_) => return Err(failure("Dimension", "expected string".into())),
        },
        pos: player.pos,
        yaw: player.yaw,
        pitch: player.pitch,
        health: player.health,
        food: player.food,
        saturation: player.saturation,
        exhaustion: player.exhaustion,
        air: player.air,
        experience_level: player.xp_level,
        experience_progress: player.xp_progress,
        experience_total: player.xp_total,
        game_mode: u8::try_from(player.game_mode).unwrap_or(0),
        selected: player.selected,
        inventory,
        carried,
        grid,
        respawn,
        respawn_dimension,
        respawn_forced,
        respawn_yaw: spawn_angle("yaw", 180.0)?,
        respawn_pitch: spawn_angle("pitch", 90.0)?,
        active_effects: active_effects_from_fields(entries),
        enchantment_seed: match find("SolarisGameplaySeed") {
            None => 0,
            Some(mc_nbt::Tag::Int(seed)) => *seed,
            Some(_) => return Err(failure("SolarisGameplaySeed", "expected integer".into())),
        },
        enchantment_offer_seed: match find("XpSeed") {
            None => 0,
            Some(mc_nbt::Tag::Int(seed)) => *seed,
            Some(_) => return Err(failure("XpSeed", "expected integer".into())),
        },
        current_impulse_impact_pos: crate::mace::read_impact(entries)
            .map_err(|error| failure("current_explosion_impact_pos", error.into()))?,
        fall_flying: match find("FallFlying") {
            None => false,
            Some(mc_nbt::Tag::Byte(0)) => false,
            Some(mc_nbt::Tag::Byte(1)) => true,
            Some(_) => return Err(failure("FallFlying", "expected boolean byte".into())),
        },
        warden_warning: match crate::warden::unique(entries, "SolarisWardenWarning")
            .map_err(|error| failure("SolarisWardenWarning", error))?
        {
            Some(mc_nbt::Tag::ByteArray(json)) => {
                let bytes: Vec<_> = json.iter().map(|byte| *byte as u8).collect();
                serde_json::from_slice(&bytes)
                    .map_err(|error| failure("SolarisWardenWarning", error.to_string()))?
            }
            Some(_) => {
                return Err(failure(
                    "SolarisWardenWarning",
                    "expected JSON byte array".into(),
                ));
            }
            None => crate::shrieker::Warning::read(entries)
                .map_err(|error| failure("warden_spawn_tracker", error))?,
        },
        burn_ticks: match find("solaris:BurnTicks") {
            Some(mc_nbt::Tag::Long(ticks)) => u32::try_from(*ticks)
                .map_err(|_| failure("solaris:BurnTicks", "expected u32".into()))?,
            Some(_) => return Err(failure("solaris:BurnTicks", "expected long".into())),
            None => entries
                .iter()
                .find_map(|(key, value)| match (key.as_str(), value) {
                    ("Fire", mc_nbt::Tag::Short(ticks)) => Some(u32::from((*ticks).max(0) as u16)),
                    _ => None,
                })
                .unwrap_or(0),
        },
        portal: crate::portals::from_fields(entries)?,
        tick: match find("solaris:Tick") {
            Some(mc_nbt::Tag::Long(tick)) => *tick as u64,
            Some(_) => return Err(failure("solaris:Tick", "expected long".into())),
            None => 0,
        },
    };
    record.statistics.advance_tick(record.tick);
    record.warden_warning.advance_tick(record.tick);
    Ok(record)
}

/// Root keys projected into the player model and regenerated on each capture.
pub(super) fn player_owned_field(key: &str) -> bool {
    matches!(
        key,
        "Pos"
            | "Rotation"
            | "Health"
            | "foodLevel"
            | "foodSaturationLevel"
            | "foodExhaustionLevel"
            | "Air"
            | "SelectedItemSlot"
            | "XpLevel"
            | "XpP"
            | "XpTotal"
            | "playerGameType"
            | "Inventory"
            | "equipment"
            | "Dimension"
            | "active_effects"
            | "AbsorptionAmount"
            | "PortalCooldown"
            | "solaris:PortalTime"
            | "XpSeed"
            | "SolarisGameplaySeed"
            | "warden_spawn_tracker"
            | "SolarisWardenWarning"
            | "current_explosion_impact_pos"
            | "FallFlying"
            | "Fire"
            | "DataVersion"
            | "SolarisStatistics"
            | "solaris:Carried"
            | "solaris:CraftingGrid"
            | "respawn"
            | "solaris:RespawnState"
            | "solaris:Tick"
            | "solaris:BurnTicks"
            | "SolarisAdvancements"
            | "SolarisUnlockedRecipes"
            | "solaris:Respawn"
            | "SpawnX"
            | "SpawnY"
            | "SpawnZ"
            | "SpawnDimension"
            | "SpawnForced"
            | "SpawnAngle"
            | "SpawnPitch"
    )
}

/// Vanilla living-entity effect keys, shared by player and mob checkpoints.
pub(crate) fn active_effects_to_fields(
    effects: &crate::effects::ActiveEffects,
) -> Vec<(String, mc_nbt::Tag)> {
    use mc_nbt::{ListTag, Tag, tag_type};
    vec![
        (
            "active_effects".into(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: active_effects_to_nbt(effects),
            }),
        ),
        ("AbsorptionAmount".into(), Tag::Float(effects.absorption)),
    ]
}

pub(crate) fn active_effects_to_nbt(effects: &crate::effects::ActiveEffects) -> Vec<mc_nbt::Tag> {
    use mc_nbt::Tag;
    fn details(effect: &crate::effects::ActiveEffect, include_id: bool) -> Tag {
        let mut fields = vec![
            ("amplifier".into(), Tag::Int(i32::from(effect.amplifier))),
            ("duration".into(), Tag::Int(effect.duration)),
            ("ambient".into(), Tag::Byte(i8::from(effect.ambient))),
            (
                "show_particles".into(),
                Tag::Byte(i8::from(effect.show_particles)),
            ),
            ("show_icon".into(), Tag::Byte(i8::from(effect.show_icon))),
        ];
        if include_id {
            fields.push((
                "id".into(),
                Tag::String(format!("minecraft:{}", effect.id.name())),
            ));
        }
        if let Some(hidden) = &effect.hidden {
            fields.push(("hidden_effect".into(), details(hidden, false)));
        }
        Tag::Compound(fields)
    }
    effects
        .entries
        .iter()
        .map(|effect| details(effect, true))
        .collect()
}

pub(crate) fn active_effects_from_fields(
    fields: &[(String, mc_nbt::Tag)],
) -> crate::effects::ActiveEffects {
    let find = |name| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, tag)| tag)
    };
    let entries = match find("active_effects") {
        Some(mc_nbt::Tag::List(list)) => list.elements.as_slice(),
        _ => &[],
    };
    let absorption = match find("AbsorptionAmount") {
        Some(mc_nbt::Tag::Float(value)) => *value,
        _ => 0.0,
    };
    active_effects_from_nbt(entries, absorption)
}

pub(crate) fn active_effects_from_nbt(
    effects: &[mc_nbt::Tag],
    absorption: f32,
) -> crate::effects::ActiveEffects {
    use crate::effects::{ActiveEffect, ActiveEffects, EffectId};
    use mc_nbt::Tag;
    fn find<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a Tag> {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, tag)| tag)
    }
    fn details(fields: &[(String, Tag)], id: EffectId) -> ActiveEffect {
        let integer = |name| match find(fields, name) {
            Some(Tag::Int(value)) => *value,
            Some(Tag::Byte(value)) => i32::from(*value as u8),
            _ => 0,
        };
        let boolean = |name, default| match find(fields, name) {
            Some(Tag::Byte(value)) => *value != 0,
            _ => default,
        };
        let show_particles = boolean("show_particles", true);
        let mut effect = ActiveEffect::new(
            id,
            integer("duration"),
            integer("amplifier").clamp(0, 255) as u8,
        );
        effect.ambient = boolean("ambient", false);
        effect.show_particles = show_particles;
        effect.show_icon = boolean("show_icon", show_particles);
        effect.hidden = match find(fields, "hidden_effect") {
            Some(Tag::Compound(hidden)) => Some(Box::new(details(hidden, id))),
            _ => None,
        };
        effect
    }
    let entries = effects
        .iter()
        .filter_map(|tag| {
            let Tag::Compound(fields) = tag else {
                return None;
            };
            let Some(Tag::String(name)) = find(fields, "id") else {
                return None;
            };
            EffectId::from_name(name).map(|id| details(fields, id))
        })
        .collect();
    ActiveEffects {
        entries,
        absorption,
    }
}
