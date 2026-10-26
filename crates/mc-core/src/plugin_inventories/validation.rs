//! Startup/replay validation of plugin inventory images.
//! Moved verbatim from `plugin_inventories.rs`.

use super::*;

pub(crate) fn validate(state: &TickState) -> Result<(), crate::config::CoreError> {
    let malformed =
        || crate::config::CoreError::Recovery("invalid native plugin inventory records".into());
    let metadata = metadata(state).map_err(|_| malformed())?;
    let mut reservations = BTreeMap::new();
    let mut actual_indexes = BTreeSet::new();
    let mut retired = BTreeMap::new();
    for (key, value) in state.plugins.domain_entries(OWNER) {
        if key == KEY {
            continue;
        }
        if let Some(endpoint) = key.strip_prefix("image/") {
            if endpoint.is_empty() {
                return Err(malformed());
            }
            let version = ImageVersion::deserialize(value).map_err(|_| malformed())?;
            ScriptInventoryFence::try_new(version.revision, version.hash)
                .map_err(|_| malformed())?;
            if version.revision == 0 {
                return Err(malformed());
            }
        } else if let Some(reference) = key.strip_prefix("reservation/") {
            reservations.insert(
                reference,
                Reservation::deserialize(value).map_err(|_| malformed())?,
            );
        } else if key.starts_with("source/")
            || key.starts_with("custody/")
            || key.starts_with("plan/")
        {
            if value != &serde_json::Value::Bool(true) {
                return Err(malformed());
            }
            actual_indexes.insert(key.to_owned());
        } else if let Some(slot) = key.strip_prefix("retired/") {
            let index = slot.parse::<usize>().map_err(|_| malformed())?;
            if index >= MAX_TERMINAL_RESERVATIONS || index.to_string() != slot {
                return Err(malformed());
            }
            retired.insert(index, value.as_str().ok_or_else(malformed)?);
        } else {
            // In particular, the old monolithic "ledger" schema is not an empty image.
            return Err(malformed());
        }
    }
    let expected_indexes: BTreeSet<_> = reservations.values().flat_map(index_keys).collect();
    let terminal: BTreeSet<_> = reservations
        .iter()
        .filter(|(_, reservation)| !protected(reservation))
        .map(|(reference, _)| *reference)
        .collect();
    let retired_refs: BTreeSet<_> = retired.values().copied().collect();
    if actual_indexes != expected_indexes
        || reservations
            .values()
            .filter(|reservation| protected(reservation))
            .count()
            != metadata.protected_count
        || retired.len() != metadata.retired_count
        || retired_refs.len() != retired.len()
        || terminal != retired_refs
        || (metadata.retired_count < MAX_TERMINAL_RESERVATIONS
            && retired.keys().copied().ne(0..metadata.retired_count))
    {
        return Err(malformed());
    }
    for (reference, reservation) in &reservations {
        if *reference != reservation.snapshot.reservation_ref
            || reservation.owner.is_empty()
            || reservation.revision == 0
            || reservation.revision > MAX_SCRIPT_WORLD_TIME
            || (reservation.snapshot.released && reservation.snapshot.bound_to.is_some())
        {
            return Err(malformed());
        }
        ScriptOperationOutcome::committed(
            reservation.revision,
            ScriptOperationPayload::OwnedInventory {
                result: Box::new(ScriptOwnedInventoryResult::Reservation {
                    reservation: reservation.snapshot.clone(),
                }),
            },
        )
        .map_err(|_| malformed())?;
        match &reservation.snapshot.endpoint {
            Endpoint::PlayerInventory { .. } => {
                if !reservation.source_cells.is_empty() {
                    return Err(malformed());
                }
                let uuid = reservation.player_uuid.ok_or_else(malformed)?;
                let live = std::iter::once(state)
                    .chain(state.other_dimensions.values().map(Box::as_ref))
                    .find_map(|target| {
                        target
                            .connections
                            .all_connections()
                            .find(|connection| {
                                target.connections.uuid_of(*connection) == Some(uuid)
                            })
                            .and_then(|connection| {
                                target
                                    .connections
                                    .entity_of(connection)
                                    .and_then(|entity| target.players.get(&entity))
                            })
                    });
                let offline = std::iter::once(state)
                    .chain(state.other_dimensions.values().map(Box::as_ref))
                    .find_map(|target| target.offline_players.get(&uuid));
                if active(reservation) {
                    let slots = live
                        .map(|player| &player.inventory.slots)
                        .or_else(|| offline.map(|player| &player.inventory));
                    if let Some(slots) = slots {
                        let candidate = &slots[..slots.len().min(36)];
                        let mut quantities = BTreeMap::new();
                        for held in reservations
                            .values()
                            .filter(|r| r.player_uuid == reservation.player_uuid && active(r))
                        {
                            for q in &held.snapshot.quantities {
                                let quantity =
                                    quantities.entry(q.resource_id.clone()).or_insert(0_u64);
                                *quantity = quantity
                                    .checked_add(q.remaining)
                                    .filter(|n| *n <= MAX_SCRIPT_WORLD_TIME)
                                    .ok_or_else(malformed)?;
                            }
                        }
                        for (resource, remaining) in quantities {
                            let id = Identifier::parse(&resource)
                                .ok()
                                .and_then(|id| state.registry.items.id_of(&id))
                                .ok_or_else(malformed)?;
                            let count: u64 = candidate
                                .iter()
                                .filter(|s| s.item_id == id && !s.is_empty())
                                .map(|s| s.count as u64)
                                .sum();
                            if count < remaining {
                                return Err(malformed());
                            }
                        }
                    } else {
                        return Err(malformed());
                    }
                }
            }
            Endpoint::Container { .. } => {
                if reservation.player_uuid.is_some() {
                    return Err(malformed());
                }
                if reservation.source_cells.is_empty() || reservation.source_cells.len() > 2 {
                    return Err(malformed());
                }
                let mut cells = BTreeSet::new();
                for cell in &reservation.source_cells {
                    ScriptBlockPosition::new(cell[0], cell[1], cell[2])
                        .validate()
                        .map_err(|_| malformed())?;
                    if !cells.insert(*cell) {
                        return Err(malformed());
                    }
                }
                match load(state, &reservation.owner, &reservation.snapshot.endpoint) {
                    Ok(image) if active(reservation) => {
                        check_floor(state, &reservation.snapshot.endpoint, &image, None)
                            .map_err(|_| malformed())?
                    }
                    Ok(_) | Err(Failure::Unloaded) => {}
                    Err(_) if !active(reservation) => {}
                    Err(_) => return Err(malformed()),
                }
            }
            _ => return Err(malformed()),
        }
    }
    Ok(())
}
