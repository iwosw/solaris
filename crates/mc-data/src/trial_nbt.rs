//! 26.1.2 trial/vault durable NBT. Field names follow the local Packed/FullConfig codecs.
use crate::item_nbt::field;
use crate::{ItemStack, items::ItemRegistry};
use mc_nbt::{ListTag, Tag, tag_type};
use std::collections::BTreeSet;
use uuid::Uuid;

pub type Fields = Vec<(String, Tag)>;

pub fn set(fields: &mut Fields, name: &str, value: Tag) {
    if let Some((_, previous)) = fields.iter_mut().find(|(key, _)| key == name) {
        *previous = value;
    } else {
        fields.push((name.into(), value));
    }
}
pub fn compound<'a>(fields: &'a Fields, name: &str) -> Result<Option<&'a Fields>, String> {
    match field(fields, name) {
        None => Ok(None),
        Some(Tag::Compound(value)) => Ok(Some(value)),
        _ => Err(format!("{name}: expected Compound")),
    }
}
pub fn long(fields: &Fields, name: &str, default: u64) -> Result<u64, String> {
    match field(fields, name) {
        None => Ok(default),
        Some(Tag::Long(value)) if *value >= 0 => Ok(*value as u64),
        _ => Err(format!("{name}: expected nonnegative Long")),
    }
}
pub fn int(fields: &Fields, name: &str, default: i32) -> Result<i32, String> {
    match field(fields, name) {
        None => Ok(default),
        Some(Tag::Int(value)) if *value >= 0 => Ok(*value),
        _ => Err(format!("{name}: expected nonnegative Int")),
    }
}
pub fn uuids(fields: &Fields, name: &str) -> Result<BTreeSet<Uuid>, String> {
    let Some(value) = field(fields, name) else {
        return Ok(BTreeSet::new());
    };
    let Tag::List(list) = value else {
        return Err(format!("{name}: expected UUID List"));
    };
    if !list.elements.is_empty() && list.element_type != tag_type::INT_ARRAY {
        return Err(format!("{name}: expected IntArray UUID entries"));
    }
    let mut result = BTreeSet::new();
    for (index, value) in list.elements.iter().enumerate() {
        let Tag::IntArray(words) = value else {
            return Err(format!("{name}[{index}]: expected IntArray[4]"));
        };
        if words.len() != 4 {
            return Err(format!("{name}[{index}]: expected IntArray[4]"));
        }
        let mut bytes = [0; 16];
        for (bytes, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            bytes.copy_from_slice(&word.to_be_bytes());
        }
        if !result.insert(Uuid::from_bytes(bytes)) {
            return Err(format!("{name}[{index}]: duplicate UUID"));
        }
    }
    Ok(result)
}
pub fn uuid_list(values: &BTreeSet<Uuid>) -> Tag {
    Tag::List(ListTag {
        element_type: tag_type::INT_ARRAY,
        elements: values
            .iter()
            .map(|uuid| {
                Tag::IntArray(
                    uuid.as_bytes()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|bytes| i32::from_be_bytes(*bytes))
                        .collect(),
                )
            })
            .collect(),
    })
}
pub fn decode(bytes: &[u8]) -> Result<Fields, String> {
    let mut input = bytes;
    let Tag::Compound(fields) =
        mc_nbt::read_network(&mut input).map_err(|error| error.to_string())?
    else {
        return Err("expected Compound".into());
    };
    if !input.is_empty() {
        return Err("trailing bytes after trial block entity".into());
    }
    let mut names = BTreeSet::new();
    for (name, _) in &fields {
        if !names.insert(name) {
            return Err(format!("{name}: duplicate field"));
        }
    }
    Ok(fields)
}
pub fn encode(mut fields: Fields, id: &str, pos: [i32; 3]) -> Vec<u8> {
    set(&mut fields, "id", Tag::String(id.into()));
    for (name, value) in ["x", "y", "z"].into_iter().zip(pos) {
        set(&mut fields, name, Tag::Int(value));
    }
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("trial NBT encodes");
    bytes
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpawnerData {
    pub fields: Fields,
    pub players: BTreeSet<Uuid>,
    pub mobs: BTreeSet<Uuid>,
    pub cooldown: u64,
    pub next_spawn: u64,
    pub total: i32,
}
impl SpawnerData {
    pub fn from_fields(fields: Fields) -> Result<Self, String> {
        let players = uuids(&fields, "registered_players")?;
        let mobs = uuids(&fields, "current_mobs")?;
        let cooldown = long(&fields, "cooldown_ends_at", 0)?;
        let next_spawn = long(&fields, "next_mob_spawns_at", 0)?;
        let total = int(&fields, "total_mobs_spawned", 0)?;
        int(&fields, "target_cooldown_length", 36_000)?;
        let range = int(&fields, "required_player_range", 14)?;
        if !(1..=128).contains(&range) {
            return Err("required_player_range: expected 1..=128".into());
        }
        if let Some(spawn) = compound(&fields, "spawn_data")? {
            validate_spawn(spawn)?;
        }
        validate_loot_string(&fields, "ejecting_loot_table")?;
        if let Some(pending) = compound(&fields, "solaris:ominous_item")? {
            if uuids(pending, "uuid")?.len() != 1 {
                return Err("solaris:ominous_item.uuid: expected exactly one UUID".into());
            }
            if field(pending, "eject_at").is_none() {
                return Err("solaris:ominous_item.eject_at: missing Long".into());
            }
            long(pending, "eject_at", 0)?;
            let item =
                compound(pending, "item")?.ok_or("solaris:ominous_item.item: missing Compound")?;
            let items = crate::items::solaris_required_items();
            if crate::item_nbt::stack_from_fields(&items, item)
                .map_err(|e| e.to_string())?
                .is_none()
            {
                return Err("solaris:ominous_item.item: empty item".into());
            }
            if !matches!(field(pending, "position"), Some(Tag::List(list)) if list.elements.len() == 3 && list.elements.iter().all(|v| matches!(v, Tag::Double(n) if n.is_finite())))
            {
                return Err("solaris:ominous_item.position: expected finite Double[3]".into());
            }
        }
        for name in ["normal_config", "ominous_config"] {
            match field(&fields, name) {
                None => {}
                Some(Tag::String(value)) => {
                    let id = crate::Identifier::parse(value).map_err(|e| format!("{name}: {e}"))?;
                    let stem = id
                        .path()
                        .strip_suffix("/normal")
                        .or_else(|| id.path().strip_suffix("/ominous"))
                        .ok_or_else(|| format!("{name}: unknown trial configuration {value}"))?;
                    if !matches!(
                        stem,
                        "trial_chamber/breeze"
                            | "trial_chamber/melee/husk"
                            | "trial_chamber/melee/spider"
                            | "trial_chamber/melee/zombie"
                            | "trial_chamber/ranged/poison_skeleton"
                            | "trial_chamber/ranged/skeleton"
                            | "trial_chamber/ranged/stray"
                            | "trial_chamber/slow_ranged/poison_skeleton"
                            | "trial_chamber/slow_ranged/skeleton"
                            | "trial_chamber/slow_ranged/stray"
                            | "trial_chamber/small_melee/baby_zombie"
                            | "trial_chamber/small_melee/cave_spider"
                            | "trial_chamber/small_melee/silverfish"
                            | "trial_chamber/small_melee/slime"
                    ) {
                        return Err(format!("{name}: unknown trial configuration {value}"));
                    }
                }
                Some(Tag::Compound(config)) => {
                    for name in [
                        "total_mobs",
                        "simultaneous_mobs",
                        "total_mobs_added_per_player",
                        "simultaneous_mobs_added_per_player",
                    ] {
                        if let Some(value) = field(config, name)
                            && !matches!(value, Tag::Float(v) if v.is_finite() && *v >= 0.0)
                        {
                            return Err(format!("{name}: expected nonnegative Float"));
                        }
                    }
                    int(config, "ticks_between_spawn", 40)?;
                    let range = int(config, "spawn_range", 4)?;
                    if !(1..=128).contains(&range) {
                        return Err("spawn_range: expected 1..=128".into());
                    }
                    validate_loot_string(config, "items_to_drop_when_ominous")?;
                    for name in ["spawn_potentials", "loot_tables_to_eject"] {
                        if let Some(value) = field(config, name) {
                            let Tag::List(list) = value else {
                                return Err(format!("{name}: expected List"));
                            };
                            let mut weight = 0_u64;
                            for (index, entry) in list.elements.iter().enumerate() {
                                let Tag::Compound(entry) = entry else {
                                    return Err(format!("{name}[{index}]: expected Compound"));
                                };
                                weight += int(entry, "weight", 1)? as u64;
                                if name == "spawn_potentials" {
                                    validate_spawn(
                                        compound(entry, "data")?
                                            .ok_or("spawn_potentials.data: missing Compound")?,
                                    )?;
                                } else {
                                    validate_loot_string(entry, "data")?;
                                    if field(entry, "data").is_none() {
                                        return Err(
                                            "loot_tables_to_eject.data: missing String".into()
                                        );
                                    }
                                }
                            }
                            if !list.elements.is_empty() && weight == 0 {
                                return Err(format!("{name}: all weights are zero"));
                            }
                        }
                    }
                }
                _ => return Err(format!("{name}: expected Compound or registry String")),
            }
        }
        Ok(Self {
            fields,
            players,
            mobs,
            cooldown,
            next_spawn,
            total,
        })
    }
    pub fn to_fields(&self) -> Fields {
        let mut fields = self.fields.clone();
        set(&mut fields, "registered_players", uuid_list(&self.players));
        set(&mut fields, "current_mobs", uuid_list(&self.mobs));
        set(
            &mut fields,
            "cooldown_ends_at",
            Tag::Long(self.cooldown as i64),
        );
        set(
            &mut fields,
            "next_mob_spawns_at",
            Tag::Long(self.next_spawn as i64),
        );
        set(&mut fields, "total_mobs_spawned", Tag::Int(self.total));
        fields
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VaultData {
    pub fields: Fields,
    pub rewarded: BTreeSet<Uuid>,
    pub items: Vec<ItemStack>,
    pub resumes: u64,
    pub total_ejections: i32,
}
impl VaultData {
    pub fn from_fields(fields: Fields, items: &ItemRegistry) -> Result<Self, String> {
        let server = compound(&fields, "server_data")?
            .cloned()
            .unwrap_or_default();
        let rewarded = uuids(&server, "rewarded_players")?;
        let resumes = long(&server, "state_updating_resumes_at", 0)?;
        let total_ejections = int(&server, "total_ejections_needed", 0)?;
        let mut output = Vec::new();
        if let Some(value) = field(&server, "items_to_eject") {
            let Tag::List(list) = value else {
                return Err("items_to_eject: expected List".into());
            };
            for (index, value) in list.elements.iter().enumerate() {
                let Tag::Compound(fields) = value else {
                    return Err(format!("items_to_eject[{index}]: expected Compound"));
                };
                let item = crate::item_nbt::stack_from_fields(items, fields)
                    .map_err(|e| format!("items_to_eject[{index}]: {e}"))?
                    .ok_or_else(|| format!("items_to_eject[{index}]: empty item"))?;
                output.push(item);
            }
        }
        if total_ejections < output.len() as i32 {
            return Err("total_ejections_needed: smaller than items_to_eject".into());
        }
        if let Some(shared) = compound(&fields, "shared_data")? {
            uuids(shared, "connected_players")?;
            if let Some(value) = field(shared, "connected_particles_range")
                && !matches!(value, Tag::Double(value) if value.is_finite() && *value >= 0.0)
            {
                return Err("connected_particles_range: expected nonnegative Double".into());
            }
            if let Some(display) = compound(shared, "display_item")? {
                crate::item_nbt::stack_from_fields(items, display)
                    .map_err(|e| format!("display_item: {e}"))?;
            }
        }
        if let Some(config) = compound(&fields, "config")? {
            validate_loot_string(config, "loot_table")?;
            validate_loot_string(config, "override_loot_table_to_display")?;
            let mut ranges = [4.0, 4.5];
            for (index, name) in ["activation_range", "deactivation_range"]
                .into_iter()
                .enumerate()
            {
                if let Some(value) = field(config, name) {
                    let Tag::Double(value) = value else {
                        return Err(format!("{name}: expected nonnegative Double"));
                    };
                    if !value.is_finite() || *value < 0.0 {
                        return Err(format!("{name}: expected nonnegative Double"));
                    }
                    ranges[index] = *value;
                }
            }
            if ranges[0] > ranges[1] {
                return Err("activation_range: greater than deactivation_range".into());
            }
            if let Some(key) = compound(config, "key_item")?
                && crate::item_nbt::stack_from_fields(items, key)
                    .map_err(|e| format!("key_item: {e}"))?
                    .is_none()
            {
                return Err("key_item: empty item".into());
            }
        }
        Ok(Self {
            fields,
            rewarded,
            items: output,
            resumes,
            total_ejections,
        })
    }
    pub fn to_fields(&self, items: &ItemRegistry) -> Fields {
        let mut fields = self.fields.clone();
        let mut server = compound(&fields, "server_data")
            .expect("validated")
            .cloned()
            .unwrap_or_default();
        set(&mut server, "rewarded_players", uuid_list(&self.rewarded));
        set(
            &mut server,
            "state_updating_resumes_at",
            Tag::Long(self.resumes as i64),
        );
        set(
            &mut server,
            "total_ejections_needed",
            Tag::Int(self.total_ejections),
        );
        set(
            &mut server,
            "items_to_eject",
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: self
                    .items
                    .iter()
                    .map(|stack| {
                        Tag::Compound(
                            crate::item_nbt::stack_to_fields(items, stack)
                                .expect("registered vault item"),
                        )
                    })
                    .collect(),
            }),
        );
        set(&mut fields, "server_data", Tag::Compound(server));
        fields
    }
}
fn validate_spawn(fields: &Fields) -> Result<(), String> {
    let entity = compound(fields, "entity")?.ok_or("spawn_data.entity: missing Compound")?;
    let Some(Tag::String(name)) = field(entity, "id") else {
        return Err("spawn_data.entity.id: missing String".into());
    };
    let id = crate::Identifier::parse(name).map_err(|e| format!("spawn_data.entity.id: {e}"))?;
    if crate::entity_types::solaris_required_entity_types()
        .id_of(&id)
        .is_none()
    {
        return Err(format!("spawn_data.entity.id: unknown entity {name}"));
    }
    Ok(())
}
fn validate_loot_string(fields: &Fields, name: &str) -> Result<(), String> {
    if let Some(value) = field(fields, name) {
        let Tag::String(table) = value else {
            return Err(format!("{name}: expected String"));
        };
        if !crate::trial_loot::contains(table) {
            return Err(format!("{name}: unknown trial loot table {table}"));
        }
    }
    Ok(())
}

/// Validate before adopting Anvil or WAL bytes; diagnostics identify the owner and field.
pub fn validate(fields: &[(String, Tag)], items: Option<&ItemRegistry>) -> Result<(), String> {
    match field(fields, "id") {
        Some(Tag::String(id)) if id == "minecraft:trial_spawner" => {
            SpawnerData::from_fields(fields.to_vec())
                .map(|_| ())
                .map_err(|e| format!("trial_spawner: {e}"))
        }
        Some(Tag::String(id)) if id == "minecraft:vault" => {
            let embedded;
            let items = match items {
                Some(items) => items,
                None => {
                    embedded = crate::items::solaris_required_items();
                    &embedded
                }
            };
            VaultData::from_fields(fields.to_vec(), items)
                .map(|_| ())
                .map_err(|e| format!("vault: {e}"))
        }
        _ => Ok(()),
    }
}
