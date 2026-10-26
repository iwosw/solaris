use mc_data::item_nbt::field;
use mc_data::trial_nbt::{self as nbt, Fields};
use mc_nbt::Tag;

#[derive(Debug, Clone, Copy)]
pub(super) struct SpawnData {
    pub type_id: u32,
    /// Block and effective sky light, inclusive. Custom rules replace placement rules.
    pub light: Option<[[u8; 2]; 2]>,
}

fn light_range(fields: &Fields, name: &str) -> Result<[u8; 2], String> {
    let pair = match field(fields, name) {
        None => [0, 15],
        Some(Tag::Int(value)) => [*value, *value],
        Some(Tag::Compound(range)) => [
            nbt::int(range, "min_inclusive", 0)?,
            nbt::int(range, "max_inclusive", 15)?,
        ],
        _ => return Err(format!("{name}: expected light range")),
    };
    if pair[0] < 0 || pair[0] > pair[1] || pair[1] > 15 {
        return Err(format!("{name}: outside light range 0..=15"));
    }
    Ok(pair.map(|value| value as u8))
}

impl SpawnData {
    /// Admits `{entity:{id}}` plus `custom_spawn_rules` light ranges (vanilla
    /// `SpawnData` is entity + custom rules + equipment). Per-entity custom NBT
    /// (beyond the id) and spawner-level `equipment` tables stay rejected:
    /// vanilla applies them at spawn time and there is no spawn-time NBT path
    /// yet, so admitting them would silently drop them.
    pub fn decode(
        fields: &Fields,
        registry: &crate::registry::CoreRegistry,
    ) -> Result<Self, String> {
        if fields
            .iter()
            .any(|(key, _)| key != "entity" && key != "custom_spawn_rules")
        {
            return Err("unsupported spawner spawn data field".into());
        }
        let entity = nbt::compound(fields, "entity")?.ok_or("SpawnData.entity missing")?;
        if entity.iter().any(|(key, _)| key != "id") {
            return Err("custom spawner entity NBT is unsupported".into());
        }
        let Some(Tag::String(name)) = field(entity, "id") else {
            return Err("SpawnData.entity.id missing".into());
        };
        let name = mc_data::Identifier::parse(name).map_err(|error| error.to_string())?;
        let type_id = registry
            .entity_types
            .id_of(&name)
            .ok_or("unknown spawner entity")?;
        registry
            .blueprint(type_id)
            .ok_or("spawner entity lacks blueprint")?;
        let light = nbt::compound(fields, "custom_spawn_rules")?
            .map(|rules| {
                Ok::<_, String>([
                    light_range(rules, "block_light_limit")?,
                    light_range(rules, "sky_light_limit")?,
                ])
            })
            .transpose()?;
        Ok(Self { type_id, light })
    }
}

pub(super) fn potentials(
    fields: &Fields,
    registry: &crate::registry::CoreRegistry,
) -> Result<Vec<(u64, SpawnData)>, String> {
    let Some(value) = field(fields, "SpawnPotentials") else {
        return Ok(Vec::new());
    };
    let Tag::List(list) = value else {
        return Err("SpawnPotentials: expected List".into());
    };
    let mut result = Vec::with_capacity(list.elements.len());
    for element in &list.elements {
        let Tag::Compound(entry) = element else {
            return Err("SpawnPotentials entry: expected Compound".into());
        };
        let weight = nbt::int(entry, "weight", 1)?;
        let data = nbt::compound(entry, "data")?.ok_or("SpawnPotentials data missing")?;
        result.push((weight as u64, SpawnData::decode(data, registry)?));
    }
    Ok(result)
}
