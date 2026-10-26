//! Plugin inventory reservations: prepare, apply and reserved debits.
//! Moved verbatim from `plugin_inventories.rs`.

use super::*;

pub(crate) fn reservation(
    state: &TickState,
    plugin_id: &str,
    reference: &str,
) -> Result<Reservation, Failure> {
    metadata(state)?;
    let mut value = stored_reservation(state, reference)?;
    if value.owner != plugin_id {
        return Err(Failure::NotFound);
    }
    // Rebind only the requested reservation, never the whole custody ledger.
    if let Some(uuid) = value.player_uuid {
        for target in std::iter::once(state).chain(state.other_dimensions.values().map(Box::as_ref))
        {
            if let Some(connection) = target
                .connections
                .all_connections()
                .find(|connection| target.connections.uuid_of(*connection) == Some(uuid))
            {
                value.snapshot.endpoint = Endpoint::PlayerInventory {
                    player_id: connection.0,
                };
                break;
            }
        }
    }
    Ok(value)
}
pub(super) struct PreparedReservation {
    pub(super) value: Reservation,
    pub(super) image: serde_json::Value,
    pub(super) metadata: Metadata,
    pub(super) old_keys: Vec<String>,
    pub(super) new_keys: Vec<String>,
    pub(super) retired: Option<(String, Option<String>)>,
    pub(super) save_metadata: bool,
}
pub(crate) fn save_reservation(state: &mut TickState, value: Reservation) -> Result<(), Failure> {
    let prepared = prepare_reservation(state, value)?;
    apply_reservation(state, prepared);
    Ok(())
}
// Resolve the retention victim and every fallible domain read before a physical debit.
pub(super) fn prepare_reservation(
    state: &TickState,
    value: Reservation,
) -> Result<PreparedReservation, Failure> {
    let mut metadata = metadata(state)?;
    let reference = &value.snapshot.reservation_ref;
    let before = match stored_reservation(state, reference) {
        Ok(before) => Some(before),
        Err(Failure::NotFound) => None,
        Err(error) => return Err(error),
    };
    let was_protected = before.as_ref().is_some_and(protected);
    let is_protected = protected(&value);
    if before.as_ref().is_some_and(|before| !protected(before)) && is_protected {
        return Err(Failure::StaleRevision);
    }
    if !was_protected && is_protected {
        if metadata.protected_count == MAX_PROTECTED_RESERVATIONS {
            return Err(Failure::Busy);
        }
        metadata.protected_count += 1;
    } else if was_protected && !is_protected {
        metadata.protected_count = metadata
            .protected_count
            .checked_sub(1)
            .ok_or(Failure::RuntimeUnavailable)?;
    }
    let retire = !is_protected && (was_protected || before.is_none());
    let retired_key = retire.then(|| format!("retired/{}", metadata.retired_next));
    let evicted = retired_key
        .as_deref()
        .and_then(|key| state.plugins.get_domain(OWNER, key))
        .map(|value| {
            value
                .as_str()
                .ok_or(Failure::RuntimeUnavailable)
                .map(str::to_owned)
        })
        .transpose()?;
    if retire && evicted.is_some() != (metadata.retired_count == MAX_TERMINAL_RESERVATIONS) {
        return Err(Failure::RuntimeUnavailable);
    }
    if let Some(evicted) = &evicted
        && protected(&stored_reservation(state, evicted)?)
    {
        return Err(Failure::RuntimeUnavailable);
    }
    let old_keys = before.as_ref().map(index_keys).unwrap_or_default();
    let new_keys = index_keys(&value);
    let save_metadata =
        retire || was_protected != is_protected || state.plugins.get_domain(OWNER, KEY).is_none();
    if retire {
        metadata.retired_next = (metadata.retired_next + 1) % MAX_TERMINAL_RESERVATIONS;
        metadata.retired_count = (metadata.retired_count + 1).min(MAX_TERMINAL_RESERVATIONS);
    }
    Ok(PreparedReservation {
        image: serde_json::to_value(&value).expect("inventory reservation serialization"),
        value,
        metadata,
        old_keys,
        new_keys,
        retired: retired_key.map(|key| (key, evicted)),
        save_metadata,
    })
}
pub(super) fn apply_reservation(
    state: &mut TickState,
    prepared: PreparedReservation,
) -> Reservation {
    let PreparedReservation {
        value,
        image,
        metadata,
        old_keys,
        new_keys,
        retired,
        save_metadata: dirty_metadata,
    } = prepared;
    for key in old_keys.iter().filter(|key| !new_keys.contains(key)) {
        state.plugins.delete_domain(OWNER, key);
    }
    for key in new_keys.iter().filter(|key| !old_keys.contains(key)) {
        state
            .plugins
            .set_domain(OWNER, key, serde_json::Value::Bool(true));
    }
    if let Some((key, evicted)) = retired {
        if let Some(evicted) = evicted {
            state
                .plugins
                .delete_domain(OWNER, &format!("reservation/{evicted}"));
        }
        state.plugins.set_domain(
            OWNER,
            &key,
            serde_json::Value::String(value.snapshot.reservation_ref.clone()),
        );
    }
    state.plugins.set_domain(
        OWNER,
        &format!("reservation/{}", value.snapshot.reservation_ref),
        image,
    );
    if dirty_metadata {
        save_metadata(state, &metadata);
    }
    value
}
pub(crate) fn validate_route_reservations(
    state: &TickState,
    plugin_id: &str,
    refs: &[String],
) -> Result<(), Failure> {
    for reference in refs {
        if reservation(state, plugin_id, reference)?.snapshot.released {
            return Err(Failure::StaleRevision);
        }
    }
    Ok(())
}
pub(crate) fn commit_reserved_debit(
    state: &mut TickState,
    plugin_id: &str,
    mut reservation: Reservation,
    materials: &BTreeMap<String, u64>,
    binding: &str,
) -> Result<(), Failure> {
    if reservation.snapshot.released
        || reservation
            .snapshot
            .bound_to
            .as_deref()
            .is_some_and(|id| id != binding)
    {
        return Err(Failure::Blocked);
    }
    if !protected(&stored_reservation(
        state,
        &reservation.snapshot.reservation_ref,
    )?) {
        return Err(Failure::StaleRevision);
    }
    let endpoint = reservation.snapshot.endpoint.clone();
    authorize_mutation(state, plugin_id, &endpoint)?;
    let image = plan_debit(state, plugin_id, &endpoint, materials)?;
    for (name, count) in materials {
        let quantity = reservation
            .snapshot
            .quantities
            .iter_mut()
            .find(|q| &q.resource_id == name)
            .ok_or(Failure::InsufficientItems)?;
        quantity.remaining = quantity
            .remaining
            .checked_sub(*count)
            .ok_or(Failure::InsufficientItems)?;
        quantity.consumed = quantity
            .consumed
            .checked_add(*count)
            .ok_or(Failure::Capacity)?;
    }
    check_floor(
        state,
        &endpoint,
        &image,
        Some(&reservation.snapshot.reservation_ref),
    )?;
    let available = stock(state, &image)?;
    let others = floor(
        state,
        &endpoint,
        Some(&reservation.snapshot.reservation_ref),
    )?;
    for quantity in &reservation.snapshot.quantities {
        let other = others.get(&quantity.resource_id).copied().unwrap_or(0);
        if available.get(&quantity.resource_id).copied().unwrap_or(0) < other + quantity.remaining {
            return Err(Failure::InsufficientItems);
        }
    }
    reservation.snapshot.bound_to = Some(binding.to_owned());
    reservation.revision = next(reservation.revision)?;
    reservation.snapshot.receipt_watermark = next(reservation.snapshot.receipt_watermark)?;
    next(image.fence.revision)?;
    let prepared = prepare_reservation(state, reservation)?;
    apply(state, plugin_id, BTreeMap::from([(endpoint, image)]))?;
    apply_reservation(state, prepared);
    Ok(())
}
