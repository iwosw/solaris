//! 26.1.2 projectile saves: vanilla keys come from the local Mojang jar's
//! Projectile, AbstractArrow, ThrowableItemProjectile, FireworkRocketEntity
//! and AreaEffectCloud addAdditionalSaveData methods. Runtime-only custody,
//! hit ledgers and timers live exclusively in a namespaced compound.

use super::{ProjectileData, ProjectileKind, ProjectileState};
use mc_data::{
    ItemStack,
    item_nbt::{stack_from_fields, stack_to_fields},
    items::ItemRegistry,
};
use mc_nbt::Tag;
use serde::Deserialize;

const RETAINED: &str = "solaris:ProjectileRuntime";

pub(crate) fn validate_state(state: &ProjectileState) -> Result<(), &'static str> {
    if !state.damage.is_finite() || state.damage < 0.0 {
        return Err("damage: expected finite nonnegative number");
    }
    if !state.acceleration_power.is_finite() || !(0.0..=1.0).contains(&state.acceleration_power) {
        return Err("acceleration_power: expected finite 0..=1");
    }
    if state.stack.count < 0 || state.stack.count > 1 {
        return Err("stack.count: expected 0 or 1");
    }
    if state.piercing > 127 {
        return Err("PierceLevel: expected 0..=127");
    }
    if state.hit.len() > 128 {
        return Err("hit: ledger exceeds 128 entries");
    }
    if state.crossbow_kills.len() > 128 {
        return Err("crossbow_kills: ledger exceeds 128 entries");
    }
    if !state.crossbow_kills.is_empty()
        && (state.fired_from_weapon.is_none()
            || !matches!(
                state.kind,
                ProjectileKind::Arrow | ProjectileKind::SpectralArrow
            ))
    {
        return Err("crossbow_kills: incompatible projectile");
    }
    for (index, hit) in state.hit.iter().enumerate() {
        if state.hit[..index].contains(hit) {
            return Err("hit: duplicate UUID");
        }
    }
    match &state.data {
        ProjectileData::Physical { .. } if !state.kind.arrow() => {
            return Err("data.Physical: incompatible native carrier");
        }
        ProjectileData::Homing { .. } if state.kind != ProjectileKind::ShulkerBullet => {
            return Err("data.Homing: incompatible kind");
        }
        ProjectileData::Cloud(cloud) => {
            if state.kind != ProjectileKind::AreaEffectCloud {
                return Err("data.Cloud: incompatible kind");
            }
            if !cloud.radius.is_finite() || cloud.radius < 0.0 {
                return Err("Radius: expected finite nonnegative number");
            }
            if !cloud.radius_on_use.is_finite() {
                return Err("RadiusOnUse: expected finite number");
            }
            if !cloud.radius_per_tick.is_finite() {
                return Err("RadiusPerTick: expected finite number");
            }
            if cloud.duration > i32::MAX as u32 && cloud.duration != u32::MAX {
                return Err("Duration: exceeds signed int");
            }
            if cloud.wait_time > i32::MAX as u32 {
                return Err("WaitTime: exceeds signed int");
            }
            if cloud.reapplication_delay == 0 || cloud.reapplication_delay > i32::MAX as u32 {
                return Err("ReapplicationDelay: expected positive signed int");
            }
        }
        ProjectileData::Fishing(_) if state.kind != ProjectileKind::FishingHook => {
            return Err("data.Fishing: incompatible kind");
        }
        ProjectileData::Firework(firework) => {
            if state.kind != ProjectileKind::Firework {
                return Err("data.Firework: incompatible kind");
            }
            if firework.lifetime > i32::MAX as u32 {
                return Err("LifeTime: exceeds signed int");
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn deserialize_optional_state<'de, D>(
    deserializer: D,
) -> Result<Option<ProjectileState>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let state = Option::<ProjectileState>::deserialize(deserializer)?;
    if let Some(state) = &state {
        validate_state(state).map_err(serde::de::Error::custom)?;
    }
    Ok(state)
}

fn uuid_tag(uuid: uuid::Uuid) -> Tag {
    Tag::IntArray(
        uuid.as_bytes()
            .chunks_exact(4)
            .map(|bytes| i32::from_be_bytes(bytes.try_into().expect("uuid word")))
            .collect(),
    )
}
fn uuid_from_tag(tag: &Tag) -> Option<uuid::Uuid> {
    let Tag::IntArray(words) = tag else {
        return None;
    };
    if words.len() != 4 {
        return None;
    }
    let mut bytes = [0; 16];
    for (target, word) in bytes.chunks_exact_mut(4).zip(words) {
        target.copy_from_slice(&word.to_be_bytes());
    }
    Some(uuid::Uuid::from_bytes(bytes))
}
fn field<'a>(fields: &'a [(String, Tag)], key: &str) -> Option<&'a Tag> {
    mc_data::item_nbt::field(fields, key)
}
fn integer(tag: &Tag) -> Option<i32> {
    match tag {
        Tag::Byte(value) => Some(i32::from(*value)),
        Tag::Short(value) => Some(i32::from(*value)),
        Tag::Int(value) => Some(*value),
        _ => None,
    }
}
fn boolean(tag: &Tag) -> Option<bool> {
    match tag {
        Tag::Byte(0) => Some(false),
        Tag::Byte(1) => Some(true),
        _ => None,
    }
}
fn number(tag: &Tag) -> Option<f32> {
    let value = match tag {
        Tag::Float(value) => *value,
        Tag::Double(value) => *value as f32,
        _ => return None,
    };
    value.is_finite().then_some(value)
}
fn item_key(kind: ProjectileKind) -> Option<&'static str> {
    match kind {
        ProjectileKind::Arrow | ProjectileKind::SpectralArrow | ProjectileKind::Trident => {
            Some("item")
        }
        ProjectileKind::Snowball
        | ProjectileKind::Egg
        | ProjectileKind::EnderPearl
        | ProjectileKind::Potion
        | ProjectileKind::LingeringPotion
        | ProjectileKind::ExperienceBottle => Some("Item"),
        ProjectileKind::Firework => Some("FireworksItem"),
        _ => None,
    }
}

/// Fields owned by the typed projection, including currently absent optionals.
pub(crate) fn owns_field(kind: ProjectileKind, key: &str) -> bool {
    matches!(key, RETAINED | "Owner" | "HasBeenShot")
        || item_key(kind) == Some(key)
        || kind.arrow()
            && matches!(
                key,
                "life" | "inGround" | "pickup" | "damage" | "crit" | "PierceLevel"
            )
        || kind == ProjectileKind::Trident && key == "DealtDamage"
        || kind == ProjectileKind::ShulkerBullet && key == "Target"
        || kind == ProjectileKind::Firework && matches!(key, "Life" | "ShotAtAngle" | "LifeTime")
        || kind == ProjectileKind::AreaEffectCloud
            && matches!(
                key,
                "Radius"
                    | "Duration"
                    | "WaitTime"
                    | "ReapplicationDelay"
                    | "RadiusOnUse"
                    | "RadiusPerTick"
                    | "potion_contents"
            )
        || matches!(
            kind,
            ProjectileKind::SmallFireball
                | ProjectileKind::Fireball
                | ProjectileKind::DragonFireball
                | ProjectileKind::WitherSkull
        ) && key == "acceleration_power"
}
fn default_stack(kind: ProjectileKind, items: &ItemRegistry) -> ItemStack {
    let name = match kind {
        ProjectileKind::Arrow => "minecraft:arrow",
        ProjectileKind::SpectralArrow => "minecraft:spectral_arrow",
        ProjectileKind::Trident => "minecraft:trident",
        ProjectileKind::Snowball => "minecraft:snowball",
        ProjectileKind::Egg => "minecraft:egg",
        ProjectileKind::EnderPearl => "minecraft:ender_pearl",
        ProjectileKind::Potion => "minecraft:splash_potion",
        ProjectileKind::LingeringPotion | ProjectileKind::AreaEffectCloud => {
            "minecraft:lingering_potion"
        }
        ProjectileKind::ExperienceBottle => "minecraft:experience_bottle",
        ProjectileKind::Firework => "minecraft:firework_rocket",
        _ => return ItemStack::EMPTY,
    };
    items
        .id_of(&mc_data::Identifier::parse(name).expect("static item identifier"))
        .map(|id| ItemStack::new(id, 1))
        .unwrap_or(ItemStack::EMPTY)
}

/// Full retained state plus the exact vanilla projection. `Item` is a shared
/// entity field; checkpoint callers move that one field into VanillaEntity.item.
pub(crate) fn to_fields(state: &ProjectileState, items: &ItemRegistry) -> Vec<(String, Tag)> {
    validate_state(state).expect("live projectile persistence invariants");
    let mut fields = vec![(
        RETAINED.into(),
        Tag::Compound(vec![
            ("version".into(), Tag::Int(1)),
            (
                "state".into(),
                Tag::String(serde_json::to_string(state).expect("projectile serde")),
            ),
        ]),
    )];
    if let ProjectileData::Homing { target } = state.data {
        fields.push(("Target".into(), uuid_tag(target)));
    }
    if let Some(owner) = state.owner {
        fields.push(("Owner".into(), uuid_tag(owner)));
    }
    if !matches!(
        state.kind,
        ProjectileKind::AreaEffectCloud | ProjectileKind::FishingHook | ProjectileKind::Lightning
    ) {
        fields.push(("HasBeenShot".into(), Tag::Byte(1)));
    }
    if matches!(
        state.kind,
        ProjectileKind::SmallFireball
            | ProjectileKind::Fireball
            | ProjectileKind::DragonFireball
            | ProjectileKind::WitherSkull
    ) {
        fields.push((
            "acceleration_power".into(),
            Tag::Double(state.acceleration_power),
        ));
    }
    if state.kind.arrow() {
        fields.extend([
            (
                "life".into(),
                Tag::Short(state.age.min(i16::MAX as u32) as i16),
            ),
            ("inGround".into(), Tag::Byte(i8::from(state.stuck))),
            ("pickup".into(), Tag::Byte(i8::from(state.pickup))),
            ("damage".into(), Tag::Double(f64::from(state.damage))),
            ("crit".into(), Tag::Byte(i8::from(state.critical))),
            ("PierceLevel".into(), Tag::Byte(state.piercing as i8)),
        ]);
        if state.kind == ProjectileKind::Trident {
            fields.push((
                "DealtDamage".into(),
                Tag::Byte(i8::from(
                    state.stuck || !state.hit.is_empty() || state.returning,
                )),
            ));
        }
    }
    if let Some(key) = item_key(state.kind).filter(|_| !state.stack.is_empty()) {
        fields.push((
            key.into(),
            Tag::Compound(stack_to_fields(items, &state.stack).expect("live projectile item")),
        ));
    }
    if state.kind == ProjectileKind::Firework {
        fields.push((
            "Life".into(),
            Tag::Int(state.age.min(i32::MAX as u32) as i32),
        ));
        fields.push((
            "ShotAtAngle".into(),
            Tag::Byte(i8::from(state.shot_at_angle)),
        ));
        if let ProjectileData::Firework(firework) = &state.data {
            fields.push(("LifeTime".into(), Tag::Int(firework.lifetime as i32)));
        }
    }
    if let ProjectileData::Cloud(cloud) = &state.data {
        fields.extend([
            ("Radius".into(), Tag::Float(cloud.radius)),
            ("Duration".into(), Tag::Int(cloud.duration as i32)),
            ("WaitTime".into(), Tag::Int(cloud.wait_time as i32)),
            (
                "ReapplicationDelay".into(),
                Tag::Int(cloud.reapplication_delay as i32),
            ),
            ("RadiusOnUse".into(), Tag::Float(cloud.radius_on_use)),
            ("RadiusPerTick".into(), Tag::Float(cloud.radius_per_tick)),
        ]);
        let stack = stack_to_fields(items, &state.stack).expect("live cloud potion");
        if let Some(Tag::Compound(components)) = field(&stack, "components")
            && let Some(contents) = field(components, "minecraft:potion_contents")
        {
            fields.push(("potion_contents".into(), contents.clone()));
        }
    }
    fields
}

/// Only error paths allocate a field path.
fn nonfinite_field(tag: &Tag) -> Option<String> {
    match tag {
        Tag::Float(value) if !value.is_finite() => Some(String::new()),
        Tag::Double(value) if !value.is_finite() => Some(String::new()),
        Tag::List(list) => {
            list.elements.iter().enumerate().find_map(|(index, tag)| {
                nonfinite_field(tag).map(|path| format!("[{index}]{path}"))
            })
        }
        Tag::Compound(fields) => fields
            .iter()
            .find_map(|(name, tag)| nonfinite_field(tag).map(|path| format!(".{name}{path}"))),
        _ => None,
    }
}

pub(crate) fn from_fields(
    fields: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<ProjectileState, String> {
    let kind = match field(fields, "id") {
        Some(Tag::String(name)) => ProjectileKind::from_name(name)
            .ok_or_else(|| "id: unknown projectile type".to_owned())?,
        None => {
            let Some(Tag::Compound(runtime)) = field(fields, RETAINED) else {
                return Err(format!("{RETAINED}: missing or not a compound"));
            };
            let Some(Tag::String(encoded)) = field(runtime, "state") else {
                return Err(format!("{RETAINED}.state: missing or not a string"));
            };
            serde_json::from_str::<ProjectileState>(encoded)
                .map_err(|err| format!("{RETAINED}.state: {err}"))?
                .kind
        }
        _ => return Err("id: expected string".into()),
    };
    from_fields_for(kind, fields, items)
}

/// Vanilla fields are authoritative; malformed present data fails closed.
pub(crate) fn from_fields_for(
    kind: ProjectileKind,
    fields: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<ProjectileState, String> {
    for (index, (name, tag)) in fields.iter().enumerate() {
        if fields[..index].iter().any(|(other, _)| other == name) {
            return Err(format!("{name}: duplicate field"));
        }
        if let Some(path) = nonfinite_field(tag) {
            return Err(format!("{name}{path}: non-finite number"));
        }
    }
    let retained = field(fields, RETAINED).is_some();
    let mut state = if let Some(runtime) = field(fields, RETAINED) {
        let Tag::Compound(runtime) = runtime else {
            return Err(format!("{RETAINED}: expected compound"));
        };
        if field(runtime, "version") != Some(&Tag::Int(1)) {
            return Err(format!("{RETAINED}.version: expected int 1"));
        }
        let Some(Tag::String(encoded)) = field(runtime, "state") else {
            return Err(format!("{RETAINED}.state: missing or not a string"));
        };
        let state: ProjectileState =
            serde_json::from_str(encoded).map_err(|err| format!("{RETAINED}.state: {err}"))?;
        if state.kind != kind {
            return Err(format!("{RETAINED}.state.kind: disagrees with id"));
        }
        validate_state(&state).map_err(|err| format!("{RETAINED}.state.{err}"))?;
        state
    } else {
        ProjectileState::new(kind, None, default_stack(kind, items))
    };
    let int = |name: &str, value: &Tag| {
        integer(value).ok_or_else(|| format!("{name}: expected byte, short or int"))
    };
    let unsigned = |name: &str, value: &Tag| {
        u32::try_from(int(name, value)?).map_err(|_| format!("{name}: negative value"))
    };
    let flag = |name: &str, value: &Tag| {
        boolean(value).ok_or_else(|| format!("{name}: expected byte 0 or 1"))
    };
    let float = |name: &str, value: &Tag| {
        number(value).ok_or_else(|| format!("{name}: expected finite float or double"))
    };
    if let Some(owner) = field(fields, "Owner") {
        state.owner =
            Some(uuid_from_tag(owner).ok_or_else(|| "Owner: expected four-int UUID".to_owned())?);
    }
    if let Some(value) = field(fields, "acceleration_power") {
        state.acceleration_power = match value {
            Tag::Double(value) => *value,
            Tag::Float(value) => f64::from(*value),
            _ => return Err("acceleration_power: expected float or double".into()),
        };
    }
    if kind.arrow() {
        if let Some(value) = field(fields, "inGround") {
            state.stuck = flag("inGround", value)?;
        }
        if let Some(value) = field(fields, "pickup") {
            let value = int("pickup", value)?;
            if !(0..=2).contains(&value) {
                return Err("pickup: expected 0..=2".into());
            }
            state.pickup = value == 1;
        }
        if let Some(value) = field(fields, "damage") {
            state.damage = float("damage", value)?;
        }
        if let Some(value) = field(fields, "crit") {
            state.critical = flag("crit", value)?;
        }
        if let Some(value) = field(fields, "PierceLevel") {
            state.piercing = unsigned("PierceLevel", value)?;
        }
        // The short vanilla life is clamped on projection; retained u32 is exact.
        if let Some(value) = field(fields, "life") {
            let age = unsigned("life", value)?;
            if !retained {
                state.age = age;
            }
        }
        if kind == ProjectileKind::Trident
            && let Some(value) = field(fields, "DealtDamage")
        {
            let returning = flag("DealtDamage", value)?;
            if !retained {
                state.returning = returning;
            }
        }
    }
    if let Some(key) = item_key(kind)
        && let Some(item) = field(fields, key)
    {
        let Tag::Compound(item) = item else {
            return Err(format!("{key}: expected item compound"));
        };
        if !matches!(field(item, "id"), Some(Tag::String(_))) {
            return Err(format!("{key}.id: missing or not a string"));
        }
        if let Some(count) = field(item, "count")
            && integer(count).is_none()
        {
            return Err(format!("{key}.count: expected byte, short or int"));
        }
        let stack = stack_from_fields(items, item)
            .map_err(|err| {
                use mc_data::item_nbt::ItemNbtError;
                match &err {
                    ItemNbtError::InvalidId(_) | ItemNbtError::UnknownItem(_) => {
                        format!("{key}.id: {err}")
                    }
                    ItemNbtError::InvalidEnchantment(_) => {
                        format!("{key}.components.minecraft:enchantments: {err}")
                    }
                    ItemNbtError::InvalidComponent(_) => format!("{key}.components: {err}"),
                }
            })?
            .ok_or_else(|| format!("{key}.count: empty custody stack"))?;
        if stack.count != 1 {
            return Err(format!("{key}.count: expected 1, got {}", stack.count));
        }
        state.stack = stack;
    }
    if kind == ProjectileKind::Firework {
        if let Some(value) = field(fields, "Life") {
            state.age = unsigned("Life", value)?;
        }
        if let Some(value) = field(fields, "ShotAtAngle") {
            state.shot_at_angle = flag("ShotAtAngle", value)?;
        }
        if let Some(value) = field(fields, "LifeTime") {
            let lifetime = unsigned("LifeTime", value)?;
            state.data = ProjectileData::Firework(super::payloads::FireworkState { lifetime });
        }
    }
    if kind == ProjectileKind::AreaEffectCloud {
        if matches!(state.data, ProjectileData::None) {
            state.data = ProjectileData::Cloud(super::payloads::CloudState {
                radius: 3.0,
                waiting: true,
                dragon: false,
                duration: u32::MAX,
                wait_time: 20,
                reapplication_delay: 20,
                radius_on_use: 0.0,
                radius_per_tick: 0.0,
                next_tick: 0,
                expires_tick: 0,
                affected_until: Default::default(),
            });
        }
        let ProjectileData::Cloud(cloud) = &mut state.data else {
            return Err(format!("{RETAINED}.state.data: expected cloud"));
        };
        if let Some(value) = field(fields, "Radius") {
            cloud.radius = float("Radius", value)?;
        }
        if let Some(value) = field(fields, "RadiusOnUse") {
            cloud.radius_on_use = float("RadiusOnUse", value)?;
        }
        if let Some(value) = field(fields, "RadiusPerTick") {
            cloud.radius_per_tick = float("RadiusPerTick", value)?;
        }
        if let Some(value) = field(fields, "Duration") {
            cloud.duration = match int("Duration", value)? {
                -1 => u32::MAX,
                value => u32::try_from(value)
                    .map_err(|_| "Duration: expected -1 or nonnegative".to_owned())?,
            };
        }
        if let Some(value) = field(fields, "WaitTime") {
            cloud.wait_time = unsigned("WaitTime", value)?;
        }
        if let Some(value) = field(fields, "ReapplicationDelay") {
            cloud.reapplication_delay = unsigned("ReapplicationDelay", value)?;
        }
        if let Some(value) = field(fields, "Age") {
            state.age = unsigned("Age", value)?;
        }
        cloud.waiting = state.age < cloud.wait_time;
        if let Some(contents) = field(fields, "potion_contents") {
            let mut stack = stack_to_fields(items, &state.stack)
                .map_err(|err| format!("potion_contents: {err}"))?;
            let component = ("minecraft:potion_contents".into(), contents.clone());
            if let Some((_, Tag::Compound(components))) =
                stack.iter_mut().find(|(key, _)| key == "components")
            {
                components.retain(|(key, _)| key != "minecraft:potion_contents");
                components.push(component);
            } else {
                stack.push(("components".into(), Tag::Compound(vec![component])));
            }
            state.stack = stack_from_fields(items, &stack)
                .map_err(|err| format!("potion_contents: {err}"))?
                .ok_or_else(|| "potion_contents: empty custody stack".to_owned())?;
        }
    }
    validate_state(&state).map_err(str::to_owned)?;
    if let Some(tag) = field(fields, "Target") {
        if kind != ProjectileKind::ShulkerBullet {
            return Err("Target: incompatible projectile kind".into());
        }
        let target = uuid_from_tag(tag).ok_or("Target: expected UUID int array")?;
        state.data = ProjectileData::Homing { target };
    }
    if !state.stack.is_empty() && items.name_of(state.stack.item_id).is_none() {
        return Err(format!(
            "{}.id: unknown custody item",
            item_key(kind).unwrap_or("stack")
        ));
    }
    Ok(state)
}

/// Shared codecs consume `Item` and `Age`; reconstruct those fields for the
/// species codec without duplicating the canonical item representation.
pub(crate) fn from_entity(
    entity: &mc_data::entity_nbt::VanillaEntity,
    items: &ItemRegistry,
) -> Result<ProjectileState, String> {
    for (name, values) in [("Pos", &entity.pos), ("Motion", &entity.motion)] {
        if let Some(index) = values.iter().position(|value| !value.is_finite()) {
            return Err(format!("{name}[{index}]: non-finite number"));
        }
    }
    if !entity.yaw.is_finite() {
        return Err("Rotation[0]: non-finite yaw".into());
    }
    if !entity.pitch.is_finite() {
        return Err("Rotation[1]: non-finite pitch".into());
    }
    let kind = ProjectileKind::from_name(&entity.id)
        .ok_or_else(|| "id: unknown projectile type".to_owned())?;
    let mut fields = entity.extra_fields.clone();
    if item_key(kind) == Some("Item")
        && let Some(stack) = &entity.item
    {
        fields.push((
            "Item".into(),
            Tag::Compound(stack_to_fields(items, stack).map_err(|err| format!("Item: {err}"))?),
        ));
    }
    if kind == ProjectileKind::AreaEffectCloud {
        fields.push(("Age".into(), Tag::Int(entity.age)));
    }
    fields.push(("id".into(), Tag::String(entity.id.clone())));
    from_fields(&fields, items)
}

/// Rebase vanilla clouds (which save age, not absolute deadlines) exactly once.
/// Native retained clocks already refer to the restored journal tick.
pub(crate) fn restore_clock(state: &mut ProjectileState, tick: u64) {
    if let ProjectileData::Cloud(cloud) = &mut state.data
        && cloud.expires_tick == 0
    {
        cloud.expires_tick = if cloud.duration == u32::MAX {
            u64::MAX
        } else {
            tick.saturating_add(
                u64::from(cloud.duration)
                    .saturating_add(u64::from(cloud.wait_time))
                    .saturating_sub(u64::from(state.age)),
            )
        };
        cloud.next_tick = tick.saturating_add(u64::from(cloud.wait_time.saturating_sub(state.age)));
    }
}

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;
