//! Read-only native POI presentation. Labels never enter the entity store/WAL.
use super::SettlementRecord;
use super::allocator::PoiKind;
use crate::boundary::BlockPos;
use crate::tick::TickState;
use crate::villager::VillagerProfession as Profession;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub struct SettlementLabel {
    /// The POI/entrance that owns this label; unique within one dimension.
    pub anchor: [i32; 3],
    pub position: [f64; 3],
    pub text: String,
}

#[derive(Default)]
pub(crate) struct Cache {
    // `PoiCache` is taken/restored by each native phase: Default must not allocate.
    current: Option<(u64, Arc<Vec<SettlementLabel>>)>,
}

/// Fixed algorithm/list, independent of process hash seeds and entity wire IDs.
pub(crate) fn villager_name(uuid: uuid::Uuid) -> &'static str {
    const NAMES: [&str; 24] = [
        "Иван",
        "Анна",
        "Пётр",
        "Мария",
        "Алексей",
        "Ольга",
        "Михаил",
        "Елена",
        "Николай",
        "Наталья",
        "Сергей",
        "Татьяна",
        "Дмитрий",
        "Ирина",
        "Андрей",
        "Дарья",
        "Василий",
        "Софья",
        "Павел",
        "Вера",
        "Фёдор",
        "Надежда",
        "Борис",
        "Любовь",
    ];
    let hash = uuid.as_bytes().iter().fold(0_u64, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u64::from(*byte))
    });
    NAMES[(hash % NAMES.len() as u64) as usize]
}

pub(crate) fn profession_name(profession: Profession) -> &'static str {
    match profession {
        Profession::None => "безработный",
        Profession::Armorer => "бронник",
        Profession::Butcher => "мясник",
        Profession::Cartographer => "картограф",
        Profession::Cleric => "священник",
        Profession::Farmer => "фермер",
        Profession::Fisherman => "рыбак",
        Profession::Fletcher => "лучник",
        Profession::Leatherworker => "кожевник",
        Profession::Librarian => "библиотекарь",
        Profession::Mason => "каменщик",
        Profession::Nitwit => "простак",
        Profession::Shepherd => "пастух",
        Profession::Toolsmith => "инструментальщик",
        Profession::Weaponsmith => "оружейник",
    }
}

fn employed(villager: &crate::villager::VillagerState, dimension: &str) -> bool {
    villager.job_site.is_some()
        && villager.job_site_dimension == dimension
        && !matches!(
            villager.data.profession,
            Profession::None | Profession::Nitwit
        )
}

fn owner_display<'a>(state: &'a TickState, record: &'a SettlementRecord) -> &'a str {
    if record.owner.is_empty() {
        return "нет";
    }
    let Ok(uuid) = uuid::Uuid::parse_str(&record.owner) else {
        return &record.owner;
    };
    for world in std::iter::once(state).chain(state.other_dimensions.values().map(|world| &**world))
    {
        for connection in world.connections.all_connections() {
            if world.connections.uuid_of(connection) == Some(uuid)
                && let Some(entity) = world.connections.entity_of(connection)
                && let Some(name) = world.connections.name_of(entity)
            {
                return name;
            }
        }
    }
    state
        .dimension_globals
        .roster
        .iter()
        .find(|player| player.uuid == uuid)
        .map_or(
            record
                .owner_name
                .as_deref()
                .unwrap_or(record.owner.as_str()),
            |player| player.name.as_str(),
        )
}

/// Same words and counts in chat and above the settlement's bell.
pub(crate) fn summary(state: &TickState, record: &SettlementRecord) -> String {
    let mut residents = 0;
    let mut workers = 0;
    for slots in state.entities.villager_chunks().values() {
        for &id in slots {
            let Some(slot) = state.entities.slot_of(id) else {
                continue;
            };
            if !state.entities.body(slot).alive {
                continue;
            }
            let Some(villager) = state.entities.row(slot).villager.as_ref() else {
                continue;
            };
            if villager.settlement == Some(record.id) {
                residents += 1;
                workers += usize::from(employed(villager, state.dimension.name()));
            }
        }
    }
    format!(
        "«{}» · глава: {} · жителей {} · работают {} · безработных {}",
        record.name,
        owner_display(state, record),
        residents,
        workers,
        residents - workers,
    )
}

fn coordinates(pos: BlockPos) -> [i32; 3] {
    [pos.x, pos.y, pos.z]
}

/// Hash only inputs which can change a label, never the simulation clock or
/// motion. Unchanged snapshots share both strings and label allocation.
fn key(state: &TickState) -> u64 {
    let mut key = std::collections::hash_map::DefaultHasher::new();
    for (pos, chunk) in &state.villager_pois.chunks {
        if chunk.pois.is_empty() {
            continue;
        }
        pos.hash(&mut key);
        chunk.revision.hash(&mut key);
        // Include neighboring chunk changes: a doorway may straddle a border.
        for x in pos.x - 1..=pos.x + 1 {
            for z in pos.z - 1..=pos.z + 1 {
                state.world.chunk_revision(x, z).hash(&mut key);
            }
        }
        for poi in &chunk.pois {
            coordinates(poi.pos).hash(&mut key);
            match poi.kind {
                PoiKind::Home => 0_i32,
                PoiKind::Bell => 1,
                PoiKind::Job(profession) => 2 + profession.profession_id(),
            }
            .hash(&mut key);
        }
    }
    for record in state.settlements.records() {
        record.id.hash(&mut key);
        record.name.hash(&mut key);
        record.owner.hash(&mut key);
        owner_display(state, record).hash(&mut key);
        record.anchor.hash(&mut key);
        record.radius.hash(&mut key);
    }
    let mut residents = 0_u64;
    for slots in state.entities.villager_chunks().values() {
        for &id in slots {
            let Some(slot) = state.entities.slot_of(id) else {
                continue;
            };
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            let Some(villager) = row.villager.as_ref() else {
                continue;
            };
            let mut resident = std::collections::hash_map::DefaultHasher::new();
            row.uuid.hash(&mut resident);
            body.alive.hash(&mut resident);
            villager.settlement.hash(&mut resident);
            villager.home.map(coordinates).hash(&mut resident);
            villager.job_site.map(coordinates).hash(&mut resident);
            villager.home_dimension.hash(&mut resident);
            villager.job_site_dimension.hash(&mut resident);
            villager.data.profession.hash(&mut resident);
            residents ^= resident.finish();
        }
    }
    residents.hash(&mut key);
    key.finish()
}

/// POIs do not carry building bounds. The nearest lower door in the bounded
/// loaded neighborhood is the entrance; outdoor worksites stay at their POI.
fn entrance(state: &TickState, poi: BlockPos) -> [i32; 3] {
    let mut nearest = None;
    for x in poi.x.saturating_sub(8)..=poi.x.saturating_add(8) {
        for z in poi.z.saturating_sub(8)..=poi.z.saturating_add(8) {
            for y in poi.y.saturating_sub(2)..=poi.y.saturating_add(2) {
                let Some(id) = state.world.block_at(x, y, z) else {
                    continue;
                };
                let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(id)) else {
                    continue;
                };
                if !block.block.id.path().ends_with("_door")
                    || !block
                        .properties
                        .iter()
                        .any(|(key, value)| key == "half" && value == "lower")
                {
                    continue;
                }
                let distance = (i64::from(x) - i64::from(poi.x)).pow(2)
                    + (i64::from(y) - i64::from(poi.y)).pow(2)
                    + (i64::from(z) - i64::from(poi.z)).pow(2);
                let candidate = (distance, [x, y, z]);
                if nearest.is_none_or(|old| candidate < old) {
                    nearest = Some(candidate);
                }
            }
        }
    }
    nearest.map_or(coordinates(poi), |(_, at)| at)
}

fn label_position(state: &TickState, anchor: [i32; 3], source: [i32; 3]) -> [f64; 3] {
    let mut position = [
        f64::from(anchor[0]) + 0.5,
        f64::from(anchor[1]) + 2.6,
        f64::from(anchor[2]) + 0.5,
    ];
    let Some(id) = state.world.block_at(anchor[0], anchor[1], anchor[2]) else {
        return position;
    };
    let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(id)) else {
        return position;
    };
    if !block.block.id.path().ends_with("_door") {
        return position;
    }
    let axis = match block
        .properties
        .iter()
        .find(|(key, _)| key == "facing")
        .map(|(_, value)| value.as_str())
    {
        Some("east" | "west") => 0,
        Some("north" | "south") => 2,
        _ => return position,
    };
    // Outside the house, beyond a one-block eave rather than inside its lintel.
    position[axis] += if source[axis] >= anchor[axis] {
        -1.75
    } else {
        1.75
    };
    position
}

#[derive(Default)]
struct Building {
    // A home's interior is a stronger outside-direction hint than a nearby job.
    source: Option<(bool, [i32; 3])>,
    homes: usize,
    occupants: usize,
    jobs: Vec<(Profession, Vec<&'static str>)>,
    bell: Option<String>,
}

fn labels(state: &TickState) -> Vec<SettlementLabel> {
    let mut buildings: BTreeMap<[i32; 3], Building> = BTreeMap::new();
    for chunk in state.villager_pois.chunks.values() {
        for poi in &chunk.pois {
            let anchor = if poi.kind == PoiKind::Bell {
                coordinates(poi.pos)
            } else {
                entrance(state, poi.pos)
            };
            let building = buildings.entry(anchor).or_default();
            let source = (poi.kind != PoiKind::Home, coordinates(poi.pos));
            if building.source.is_none_or(|old| source < old) {
                building.source = Some(source);
            }
            match poi.kind {
                PoiKind::Home => {
                    building.homes += 1;
                    building.occupants += residents_at(state, poi.pos, false).count();
                }
                PoiKind::Job(profession) => {
                    let mut names: Vec<_> = residents_at(state, poi.pos, true)
                        .map(villager_name)
                        .collect();
                    names.sort_unstable();
                    building.jobs.push((profession, names));
                }
                PoiKind::Bell => {
                    let record = state
                        .settlements
                        .records()
                        .filter(|record| {
                            anchor
                                .iter()
                                .zip(record.anchor)
                                .map(|(at, centre)| (i64::from(*at) - i64::from(centre)).pow(2))
                                .sum::<i64>()
                                <= i64::from(record.radius).pow(2)
                        })
                        .min_by_key(|record| {
                            (
                                anchor
                                    .iter()
                                    .zip(record.anchor)
                                    .map(|(at, centre)| (i64::from(*at) - i64::from(centre)).pow(2))
                                    .sum::<i64>(),
                                record.id,
                            )
                        });
                    building.bell = Some(record.map_or_else(
                        || "Деревня (без хозяина) · /settlement create <name>".into(),
                        |record| summary(state, record),
                    ));
                }
            }
        }
    }
    buildings
        .into_iter()
        .map(|(anchor, mut building)| {
            let mut lines = Vec::new();
            if building.homes != 0 {
                lines.push(format!(
                    "Дом · жильцов {}/{}",
                    building.occupants, building.homes
                ));
            }
            building
                .jobs
                .sort_by(|(left, left_names), (right, right_names)| {
                    left.profession_id()
                        .cmp(&right.profession_id())
                        .then_with(|| left_names.cmp(right_names))
                });
            for (profession, names) in building.jobs {
                let title = if profession == Profession::Farmer {
                    "Ферма"
                } else {
                    "Мастерская"
                };
                lines.push(format!(
                    "{title} · {} {}",
                    profession_name(profession),
                    if names.is_empty() {
                        "нет".into()
                    } else {
                        names.join(", ")
                    }
                ));
            }
            if let Some(bell) = building.bell {
                lines.push(bell);
            }
            SettlementLabel {
                anchor,
                position: label_position(
                    state,
                    anchor,
                    building.source.map_or(anchor, |(_, source)| source),
                ),
                text: lines.join("\n"),
            }
        })
        .collect()
}

fn residents_at(
    state: &TickState,
    poi: BlockPos,
    job: bool,
) -> impl Iterator<Item = uuid::Uuid> + '_ {
    state
        .entities
        .villager_chunks()
        .values()
        .flatten()
        .filter_map(move |&id| {
            let slot = state.entities.slot_of(id)?;
            let body = state.entities.body(slot);
            let villager = state.entities.row(slot).villager.as_ref()?;
            let (at, dimension) = if job {
                (villager.job_site, &villager.job_site_dimension)
            } else {
                (villager.home, &villager.home_dimension)
            };
            (body.alive && at == Some(poi) && dimension == state.dimension.name())
                .then_some(state.entities.row(slot).uuid)
        })
}

pub(crate) fn snapshot(state: &mut TickState) -> Arc<Vec<SettlementLabel>> {
    let key = key(state);
    if state
        .villager_pois
        .presentation
        .current
        .as_ref()
        .is_none_or(|(old, _)| *old != key)
    {
        let labels = Arc::new(labels(state));
        state.villager_pois.presentation.current = Some((key, labels));
    }
    Arc::clone(
        &state
            .villager_pois
            .presentation
            .current
            .as_ref()
            .expect("initialized label snapshot")
            .1,
    )
}

#[cfg(test)]
#[path = "presentation_tests.rs"]
mod tests;
