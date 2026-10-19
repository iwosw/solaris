//! Tick-owned plugin player effects. Durable zones and storage use the native
//! plugin image; menus and memberships are connection-local and never replayed.
use std::collections::{BTreeMap, BTreeSet};

use mc_data::{Identifier, ItemStack};
use mc_script::{
    AdmittedScriptCommand, ScriptAxisAlignedZone, ScriptCommand, ScriptEvent, ScriptInventoryClick,
    ScriptInventoryEndpoint, ScriptInventoryMenu, ScriptInventoryResourceDelta,
    ScriptOperationFailure, ScriptPlayerContext, ScriptPlayerId, ScriptPlayerInventoryFailure,
    ScriptPluginTarget, ScriptPosition, ScriptZoneProtection,
};
use mc_world::BlockPos;
use serde::{Deserialize, Serialize};

use crate::boundary::{ClickMode, ConnectionId, EntityId, PlayerChange, PlayerInput};
use crate::tick::TickState;

const ZONES: &str = "player-zones";

#[derive(Default)]
pub(crate) struct PlayerAuthority {
    menus: BTreeMap<ConnectionId, OpenMenu>,
    zones: BTreeMap<(String, String), Zone>,
    permissions: BTreeMap<ConnectionId, u8>,
    loader_ready: BTreeSet<ConnectionId>,
}

impl PlayerAuthority {
    pub(crate) fn observe_permission(&mut self, connection: ConnectionId, level: u8) {
        self.permissions.insert(connection, level);
    }

    pub(crate) fn is_operator(&self, connection: ConnectionId) -> bool {
        self.permissions
            .get(&connection)
            .is_some_and(|level| *level >= 2)
    }
}

/// Only the edge's successful negotiated Loader bind may call this hook.
pub(crate) fn note_loader_ready(state: &mut TickState, connection: ConnectionId) {
    if state.connections.entity_of(connection).is_some() {
        state.plugin_players.loader_ready.insert(connection);
    }
}

struct OpenMenu {
    target: ScriptPluginTarget,
    definition: ScriptInventoryMenu,
    window: u8,
    items: Vec<ItemStack>,
}

struct Zone {
    definition: ScriptAxisAlignedZone,
    members: BTreeSet<ConnectionId>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ZoneRecord {
    dimension: String,
    minimum: [f64; 3],
    maximum: [f64; 3],
    allowed_actor: Option<String>,
}

impl ZoneRecord {
    fn from_zone(zone: &ScriptAxisAlignedZone) -> Self {
        let a = zone.minimum();
        let b = zone.maximum();
        Self {
            dimension: zone.dimension().into(),
            minimum: [a.x(), a.y(), a.z()],
            maximum: [b.x(), b.y(), b.z()],
            allowed_actor: zone
                .protection()
                .map(|policy| policy.allowed_actor_uuid().into()),
        }
    }

    fn into_zone(self, id: &str) -> Result<ScriptAxisAlignedZone, crate::CoreError> {
        let invalid = || crate::CoreError::Storage("native plugins: invalid player zone".into());
        if crate::dimensions::Dimension::parse(&self.dimension).is_none() {
            return Err(invalid());
        }
        let minimum = ScriptPosition::try_new(self.minimum[0], self.minimum[1], self.minimum[2])
            .ok_or_else(invalid)?;
        let maximum = ScriptPosition::try_new(self.maximum[0], self.maximum[1], self.maximum[2])
            .ok_or_else(invalid)?;
        let protection = self
            .allowed_actor
            .map(ScriptZoneProtection::try_actor_or_operator)
            .transpose()
            .map_err(|_| invalid())?;
        ScriptAxisAlignedZone::try_new_with_protection(
            id,
            &self.dimension,
            minimum,
            maximum,
            protection,
        )
        .map_err(|_| invalid())
    }
}

/// Called after WAL/checkpoint restoration, before plugin commands or player inputs.
pub(crate) fn restore(state: &mut TickState) -> Result<(), crate::CoreError> {
    let mut zones = BTreeMap::new();
    for owner in state.plugins.domain_plugins() {
        let Some(image) = state.plugins.get_domain(owner, ZONES) else {
            continue;
        };
        let records: BTreeMap<String, ZoneRecord> =
            serde_json::from_value(image.clone()).map_err(|error| {
                crate::CoreError::Storage(format!("native plugins: zones: {error}"))
            })?;
        for (id, record) in records {
            zones.insert(
                (owner.into(), id.clone()),
                Zone {
                    definition: record.into_zone(&id)?,
                    members: BTreeSet::new(),
                },
            );
        }
    }
    state.plugin_players.zones = zones;
    Ok(())
}

fn persist_zones(state: &mut TickState, owner: &str) {
    let records: BTreeMap<_, _> = state
        .plugin_players
        .zones
        .iter()
        .filter(|((plugin, _), _)| plugin == owner)
        .map(|((_, id), zone)| (id, ZoneRecord::from_zone(&zone.definition)))
        .collect();
    if records.is_empty() {
        state.plugins.delete_domain(owner, ZONES);
    } else {
        state.plugins.set_domain(
            owner,
            ZONES,
            serde_json::to_value(records).expect("validated finite zone coordinates serialize"),
        );
    }
}

/// Executes only the admitted player-authority command families.
pub(crate) fn execute(
    state: &mut TickState,
    admitted: AdmittedScriptCommand,
) -> Option<ScriptEvent> {
    let session = match admitted.request() {
        ScriptCommand::InventoryStorageTransaction { transaction } => Some(transaction.player_id()),
        ScriptCommand::PlayerInventoryTransaction { transaction } => Some(transaction.player_id()),
        ScriptCommand::OpenInventoryMenu { player_id, .. }
        | ScriptCommand::CloseInventoryMenu { player_id, .. }
        | ScriptCommand::ClientSound { player_id, .. }
        | ScriptCommand::UnlockSession { player_id } => Some(*player_id),
        _ => None,
    };
    if let Some(session) = session {
        let connection = ConnectionId(session.value());
        if state.connections.entity_of(connection).is_none()
            && let Some(dimension) = state
                .other_dimensions
                .iter()
                .find(|(_, world)| world.connections.entity_of(connection).is_some())
                .map(|(dimension, _)| *dimension)
        {
            let mut world = state
                .other_dimensions
                .remove(&dimension)
                .expect("resolved dimension remains tick-owned");
            crate::dimensions::swap_globals(state, &mut world);
            let result = execute_local(&mut world, admitted);
            crate::dimensions::swap_globals(state, &mut world);
            state.other_dimensions.insert(dimension, world);
            return result;
        }
    }
    execute_local(state, admitted)
}

fn execute_local(state: &mut TickState, admitted: AdmittedScriptCommand) -> Option<ScriptEvent> {
    match admitted.request() {
        ScriptCommand::InventoryStorageTransaction { transaction } => {
            let owner = admitted.plugin_id();
            let committed = state.plugins.check_storage(owner, transaction.storage())
                && transact(
                    state,
                    owner,
                    transaction.player_id(),
                    transaction.inventory(),
                )
                .is_ok();
            if committed {
                state.plugins.apply_storage(owner, transaction.storage());
            }
            admitted
                .inventory_storage_transaction_result(committed)
                .ok()
        }
        ScriptCommand::PlayerInventoryTransaction { transaction } => {
            let failure = transact(
                state,
                admitted.plugin_id(),
                transaction.player_id(),
                transaction.deltas(),
            )
            .err();
            admitted.player_inventory_transaction_result(failure).ok()
        }
        ScriptCommand::OpenInventoryMenu { .. } => {
            let (target, player, menu) = admitted.into_open_inventory_menu().ok()?;
            open_menu(state, target, player, menu);
            None
        }
        ScriptCommand::CloseInventoryMenu { player_id, menu_id } => {
            let connection = ConnectionId(player_id.value());
            if state
                .plugin_players
                .menus
                .get(&connection)
                .is_some_and(|menu| {
                    menu.target.plugin_id() == admitted.plugin_id()
                        && menu.definition.id() == menu_id
                })
            {
                close_menu(state, connection);
            }
            None
        }
        ScriptCommand::UnlockSession { player_id } => {
            // Exact-session scoped: the helper removes the lock iff the fence
            // is enabled and finishes deferred restored custody.
            crate::tick::unlock_session(state, ConnectionId(player_id.value()));
            None
        }
        ScriptCommand::UpsertZone { .. } => {
            let (target, definition) = admitted.into_upsert_zone().ok()?;
            let key = (target.plugin_id().to_owned(), definition.id().to_owned());
            let accepted = crate::dimensions::Dimension::parse(definition.dimension()).is_some();
            let result = target.zone_command_result(definition.id(), accepted).ok();
            if accepted {
                // Preserve memberships: changed bounds emit transitions on the next tick.
                let members = state
                    .plugin_players
                    .zones
                    .remove(&key)
                    .map(|old| old.members)
                    .unwrap_or_default();
                let owner = key.0.clone();
                state.plugin_players.zones.insert(
                    key,
                    Zone {
                        definition,
                        members,
                    },
                );
                persist_zones(state, &owner);
            }
            result
        }
        ScriptCommand::RemoveZone { .. } => {
            let (target, id) = admitted.into_remove_zone().ok()?;
            let key = (target.plugin_id().to_owned(), id.clone());
            if let Some(zone) = state.plugin_players.zones.remove(&key) {
                for connection in zone.members {
                    if let Some(context) = context(state, connection)
                        && let Ok(event) = target.player_zone_exited(
                            ScriptPlayerId::new(connection.0),
                            context,
                            &zone.definition,
                        )
                        && let Some(runtime) = state.plugin_runtime.as_mut()
                    {
                        runtime.boundary.enqueue_observation(event);
                    }
                }
                persist_zones(state, target.plugin_id());
            }
            target.zone_command_result(id, true).ok()
        }
        ScriptCommand::ClientSound { .. } => {
            let (target, player, sound) = admitted.into_client_sound().ok()?;
            let connection = ConnectionId(player.value());
            if state.connections.entity_of(connection).is_some()
                && state.plugin_players.loader_ready.contains(&connection)
                && sound
                    .sound_id()
                    .strip_prefix(target.plugin_id())
                    .is_some_and(|suffix| suffix.starts_with(':') && suffix.len() > 1)
                && state
                    .plugin_runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.client_sound_allowed(target.plugin_id(), &sound))
            {
                state.push_player(
                    connection,
                    PlayerChange::PluginClientSound {
                        plugin_id: target.plugin_id().into(),
                        sound,
                    },
                );
            } else {
                tracing::warn!(
                    plugin = target.plugin_id(),
                    session = connection.0,
                    sound = sound.sound_id(),
                    "plugin client sound refused: session or negotiated permission unavailable"
                );
            }
            None
        }
        ScriptCommand::OpenClientView { request } => {
            tracing::error!(
                plugin = admitted.plugin_id(),
                command = "open-client-view",
                "plugin command refused: native view authority unavailable"
            );
            ScriptEvent::client_view_opened(
                admitted.plugin_id(),
                request.request_id(),
                request.player_id(),
                None,
                None,
                Some(mc_script::ScriptClientViewFailure::Refused),
            )
            .ok()
        }
        ScriptCommand::BeginClientSelection {
            request_id,
            player_id,
            ..
        } => {
            tracing::error!(
                plugin = admitted.plugin_id(),
                command = "begin-client-selection",
                "plugin command refused: native selection authority unavailable"
            );
            ScriptEvent::client_selection_started(
                admitted.plugin_id(),
                request_id,
                *player_id,
                None,
                None,
                Some(mc_script::ScriptClientViewFailure::Refused),
            )
            .ok()
        }
        ScriptCommand::GrantLoaderBlockItem { .. } => {
            tracing::error!(
                plugin = admitted.plugin_id(),
                command = "grant-loader-block-item",
                "plugin command refused: native Loader item authority unavailable"
            );
            admitted
                .loader_item_grant_result(Some(
                    mc_script::ScriptLoaderItemGrantFailure::RuntimeUnavailable,
                ))
                .ok()
        }
        _ => {
            let family = match admitted.request() {
                ScriptCommand::PresentClientView { .. } => "present-client-view",
                ScriptCommand::CloseClientView { .. } => "close-client-view",
                ScriptCommand::CancelClientSelection { .. } => "cancel-client-selection",
                ScriptCommand::SendCustomPayload { .. } => "send-custom-payload",
                _ => "unhandled-player-authority-command",
            };
            tracing::error!(
                plugin = admitted.plugin_id(),
                command = family,
                "plugin command refused: native command authority unavailable"
            );
            None
        }
    }
}

fn transact(
    state: &mut TickState,
    owner: &str,
    player: ScriptPlayerId,
    deltas: &[ScriptInventoryResourceDelta],
) -> Result<(), ScriptPlayerInventoryFailure> {
    let endpoint = ScriptInventoryEndpoint::PlayerInventory {
        player_id: player.value(),
    };
    if state
        .connections
        .entity_of(ConnectionId(player.value()))
        .is_none()
    {
        return Err(ScriptPlayerInventoryFailure::PlayerUnavailable);
    }
    // Resolve all resources before touching either authority, even debit-only requests.
    let resources: Vec<_> = deltas
        .iter()
        .map(|delta| {
            Identifier::parse(delta.resource_id())
                .ok()
                .and_then(|resource| state.registry.items.id_of(&resource))
                .ok_or(ScriptPlayerInventoryFailure::UnknownResource)
        })
        .collect::<Result<_, _>>()?;
    let debits = deltas
        .iter()
        .filter(|delta| delta.delta() < 0)
        .map(|delta| {
            (
                delta.resource_id().to_owned(),
                u64::from(delta.delta().unsigned_abs()),
            )
        })
        .collect();
    let mut image = crate::plugin_inventories::plan_debit(state, owner, &endpoint, &debits)
        .map_err(inventory_failure)?;
    for (delta, resource) in deltas.iter().zip(resources) {
        if delta.delta() > 0 {
            crate::plugin_inventories::insert(
                state,
                &mut image,
                ItemStack::new(resource, i32::from(delta.delta())),
            )
            .map_err(inventory_failure)?;
        }
    }
    crate::plugin_inventories::commit(state, owner, BTreeMap::from([(endpoint, image)]))
        .map_err(inventory_failure)?;
    refresh_menu(state, ConnectionId(player.value()));
    Ok(())
}

fn inventory_failure(failure: ScriptOperationFailure) -> ScriptPlayerInventoryFailure {
    match failure {
        ScriptOperationFailure::NotFound => ScriptPlayerInventoryFailure::PlayerUnavailable,
        ScriptOperationFailure::InsufficientItems => {
            ScriptPlayerInventoryFailure::InsufficientResource
        }
        ScriptOperationFailure::Capacity => ScriptPlayerInventoryFailure::InventoryFull,
        _ => ScriptPlayerInventoryFailure::RuntimeUnavailable,
    }
}

fn open_menu(
    state: &mut TickState,
    target: ScriptPluginTarget,
    player: ScriptPlayerId,
    definition: ScriptInventoryMenu,
) {
    let connection = ConnectionId(player.value());
    let Some(entity) = state.connections.entity_of(connection) else {
        return;
    };
    let rows = definition
        .slots()
        .iter()
        .map(|slot| usize::from(slot.index()) / 9 + 1)
        .max()
        .unwrap_or(1);
    let mut items = vec![ItemStack::EMPTY; rows * 9];
    for slot in definition.slots() {
        let Some(resource) = Identifier::parse(slot.item().resource_id())
            .ok()
            .and_then(|resource| state.registry.items.id_of(&resource))
        else {
            return;
        };
        let mut item = ItemStack::new(resource, i32::from(slot.item().count()));
        item.custom_name = slot.item().label().map(|label| {
            serde_json::json!({"text": label})
                .to_string()
                .into_boxed_str()
        });
        items[usize::from(slot.index())] = item;
    }
    let registry = state.registry.clone();
    let Some(native) = state.players.get_mut(&entity) else {
        return;
    };
    let effects = native.open_menu(
        &registry.item_rules(),
        crate::inventory::MenuKind::Chest { rows: rows as u8 },
        serde_json::json!({"text": definition.title()}).to_string(),
        &items,
    );
    let Some(window) = native.open.as_ref().map(|menu| menu.window) else {
        return;
    };
    state.open_containers.remove(&entity);
    publish_effects(state, connection, entity, effects);
    state.plugin_players.menus.insert(
        connection,
        OpenMenu {
            target,
            definition,
            window,
            items,
        },
    );
}

fn publish_effects(
    state: &mut TickState,
    connection: ConnectionId,
    entity: EntityId,
    effects: crate::player::PlayerEffects,
) {
    for change in effects.changes {
        state.push_player(connection, change);
    }
    if let Some(slot) = state.entities.slot_of(entity) {
        let position = state.entities.pos(slot);
        state
            .merge_life_drops
            .extend(effects.dropped.into_iter().map(|stack| (position, stack)));
    }
}

fn close_menu(state: &mut TickState, connection: ConnectionId) {
    let Some(open) = state.plugin_players.menus.remove(&connection) else {
        return;
    };
    let Some(entity) = state.connections.entity_of(connection) else {
        return;
    };
    let registry = state.registry.clone();
    let Some(player) = state.players.get_mut(&entity) else {
        return;
    };
    if player
        .open
        .as_ref()
        .is_none_or(|menu| menu.window != open.window)
    {
        return;
    }
    let effects = player.close(&registry.item_rules());
    state.push_player(
        connection,
        PlayerChange::MenuClosed {
            window: open.window,
        },
    );
    publish_effects(state, connection, entity, effects);
}

fn refresh_menu(state: &mut TickState, connection: ConnectionId) {
    let Some(entity) = state.connections.entity_of(connection) else {
        return;
    };
    let Some(menu) = state.plugin_players.menus.get(&connection) else {
        return;
    };
    let Some(player) = state.players.get(&entity) else {
        return;
    };
    let changes = player.sync_container_view(&menu.items);
    for change in changes {
        state.push_player(connection, change);
    }
}

/// Consume plugin-menu packets before vanilla custody or prediction can move icons.
pub(crate) fn menu_input(
    state: &mut TickState,
    input: &PlayerInput,
) -> (bool, Option<ScriptEvent>) {
    let connection = input.connection();
    if matches!(input, PlayerInput::Leave { .. }) {
        // Leave is not dimension travel: discard only the disconnected session.
        state.plugin_players.menus.remove(&connection);
        state.plugin_players.permissions.remove(&connection);
        state.plugin_players.loader_ready.remove(&connection);
        for zone in state.plugin_players.zones.values_mut() {
            zone.members.remove(&connection);
        }
        return (false, None);
    }
    let Some(menu) = state.plugin_players.menus.get(&connection) else {
        return (false, None);
    };
    let Some(entity) = state.connections.entity_of(connection) else {
        return (false, None);
    };
    if state
        .players
        .get(&entity)
        .and_then(|player| player.open.as_ref())
        .is_none_or(|native| native.window != menu.window)
    {
        state.plugin_players.menus.remove(&connection);
        return (false, None);
    }
    match input {
        PlayerInput::ContainerClose { .. } => {
            close_menu(state, connection);
            (true, None)
        }
        PlayerInput::ContainerClick {
            window,
            state_id,
            slot,
            button,
            mode,
            ..
        } => {
            let native = state.players[&entity]
                .open
                .as_ref()
                .expect("matched plugin menu");
            let click = match (mode, button) {
                (ClickMode::Pickup, 0) => Some(ScriptInventoryClick::Primary),
                (ClickMode::Pickup, 1) => Some(ScriptInventoryClick::Secondary),
                (ClickMode::QuickMove, 0) => Some(ScriptInventoryClick::ShiftPrimary),
                (ClickMode::QuickMove, 1) => Some(ScriptInventoryClick::ShiftSecondary),
                _ => None,
            };
            let event = if *window == menu.window && *state_id == native.state_id {
                u8::try_from(*slot)
                    .ok()
                    .filter(|slot| {
                        menu.definition
                            .slots()
                            .iter()
                            .any(|icon| icon.index() == *slot)
                    })
                    .zip(click)
                    .and_then(|(slot, click)| {
                        context(state, connection).and_then(|context| {
                            menu.target
                                .inventory_menu_clicked(
                                    ScriptPlayerId::new(connection.0),
                                    context,
                                    &menu.definition,
                                    slot,
                                    click,
                                )
                                .ok()
                        })
                    })
            } else {
                None
            };
            refresh_menu(state, connection);
            (true, event)
        }
        PlayerInput::ContainerButton { .. }
        | PlayerInput::PlaceRecipe { .. }
        | PlayerInput::RenameItem { .. }
        | PlayerInput::ContainerSlotState { .. }
        | PlayerInput::SelectTrade { .. } => (true, None),
        _ => (false, None),
    }
}

fn context(state: &TickState, connection: ConnectionId) -> Option<ScriptPlayerContext> {
    let world = if state.connections.entity_of(connection).is_some() {
        state
    } else {
        state
            .other_dimensions
            .values()
            .find(|world| world.connections.entity_of(connection).is_some())?
            .as_ref()
    };
    let entity = world.connections.entity_of(connection)?;
    let slot = world.entities.slot_of(entity)?;
    let position = world.entities.pos(slot);
    ScriptPlayerContext::try_new(
        world.connections.uuid_of(connection)?.to_string(),
        world.connections.name_of(entity)?,
        state.plugin_players.is_operator(connection),
        position[0],
        position[1],
        position[2],
    )
    .ok()
}

fn contains(zone: &ScriptAxisAlignedZone, dimension: &str, position: [f64; 3]) -> bool {
    let minimum = zone.minimum();
    let maximum = zone.maximum();
    zone.dimension() == dimension
        && position[0] >= minimum.x()
        && position[0] <= maximum.x()
        && position[1] >= minimum.y()
        && position[1] <= maximum.y()
        && position[2] >= minimum.z()
        && position[2] <= maximum.z()
}

/// Source-owned world mutations may not cross another plugin's protected zone.
/// Event-only zones do not claim custody; a plugin may mutate its own zones.
pub(crate) fn allows_plugin_bounds(
    state: &TickState,
    owner: &str,
    dimension: &str,
    minimum: [i32; 3],
    maximum: [i32; 3],
) -> bool {
    state
        .plugin_players
        .zones
        .iter()
        .all(|((zone_owner, _), zone)| {
            if zone_owner == owner
                || zone.definition.dimension() != dimension
                || zone.definition.protection().is_none()
            {
                return true;
            }
            let low = zone.definition.minimum();
            let high = zone.definition.maximum();
            f64::from(maximum[0]) < low.x()
                || f64::from(minimum[0]) > high.x()
                || f64::from(maximum[1]) < low.y()
                || f64::from(minimum[1]) > high.y()
                || f64::from(maximum[2]) < low.z()
                || f64::from(minimum[2]) > high.z()
        })
}

/// Native break/place/use admission must call this for the actual affected cell.
pub(crate) fn allows_block(state: &TickState, actor: EntityId, position: BlockPos) -> bool {
    if !state
        .plugin_players
        .zones
        .values()
        .any(|zone| zone.definition.protection().is_some())
    {
        return true;
    }
    let Some(connection) = state.connections.entity_connection(actor) else {
        return true;
    };
    let Some(uuid) = state.connections.uuid_of(connection) else {
        return false;
    };
    let uuid = uuid.to_string();
    state.plugin_players.zones.values().all(|zone| {
        !contains(
            &zone.definition,
            state.dimension.name(),
            [
                f64::from(position.x),
                f64::from(position.y),
                f64::from(position.z),
            ],
        ) || zone.definition.protection().is_none_or(|policy| {
            policy.allows_actor(&uuid, state.plugin_players.is_operator(connection))
        })
    })
}

/// Called after authoritative player movement and dimension travel, before publish.
pub(crate) fn tick(state: &mut TickState) -> Vec<ScriptEvent> {
    // The global authority is swapped through dimension-local ticks. An absent
    // connection may be alive in another dimension; only inspect local players.
    state.plugin_players.menus.retain(|connection, menu| {
        state
            .connections
            .entity_of(*connection)
            .is_none_or(|entity| {
                state
                    .players
                    .get(&entity)
                    .and_then(|player| player.open.as_ref())
                    .is_some_and(|native| native.window == menu.window)
            })
    });
    if state.plugin_players.zones.is_empty() || state.plugin_runtime.is_none() {
        return Vec::new();
    }
    let contexts: BTreeMap<_, _> = state
        .connections
        .all_connections()
        .filter_map(|connection| context(state, connection).map(|context| (connection, context)))
        .collect();
    let mut events = Vec::new();
    for ((owner, _), zone) in &mut state.plugin_players.zones {
        let Some(target) = state
            .plugin_runtime
            .as_ref()
            .and_then(|runtime| runtime.boundary.plugin_target(owner))
        else {
            continue;
        };
        for (connection, context) in &contexts {
            let inside = contains(
                &zone.definition,
                state.dimension.name(),
                [context.x(), context.y(), context.z()],
            );
            let was_inside = zone.members.contains(connection);
            if inside == was_inside {
                continue;
            }
            let event = if inside {
                zone.members.insert(*connection);
                target.player_zone_entered(
                    ScriptPlayerId::new(connection.0),
                    context.clone(),
                    &zone.definition,
                )
            } else {
                zone.members.remove(connection);
                target.player_zone_exited(
                    ScriptPlayerId::new(connection.0),
                    context.clone(),
                    &zone.definition,
                )
            };
            if let Ok(event) = event {
                events.push(event);
            }
        }
    }
    events
}

#[cfg(test)]
#[path = "plugin_player_tests.rs"]
mod tests;
