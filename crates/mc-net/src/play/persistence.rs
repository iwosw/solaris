//! Runtime player persistence state and its adapter to the simulation-owned
//! playerdata codec.

use std::fmt;
use std::path::Path;

use mc_data::ItemStack;
use mc_data::items::ItemRegistry;
use mc_domain::GameMode;
use mc_protocol::packets::play::{ClientboundSetExperience, MovePlayerFlags};
use mc_simulation::inventory::PlayerInventory;
use mc_simulation::persistence::inventory_recovery::PlayerOwnedInventory;
use mc_simulation::persistence::{
    PersistedPlayerPose, PlayerInventoryExtras, PlayerPersistedRecord, PlayerPersistenceError,
    SpawnState, XpState, playerdata_path,
};
use mc_simulation::survival::SurvivalState;

use super::PlayerPose;
use super::session::player_effects::PlayerEffectsState;

#[derive(Debug, Clone)]
pub(crate) struct PlayerPersistedState {
    pub(super) pose: PlayerPose,
    pub(super) game_mode: GameMode,
    pub(super) survival: SurvivalState,
    pub(super) effects: Option<PlayerEffectsState>,
    pub(super) physical_stamina: Option<mc_simulation::combat::PhysicalStamina>,
    pub(super) physical_hurt_resistance: Option<mc_simulation::combat::PlayerHurtResistance>,
    pub(super) physical_shield_disabled_until: Option<u64>,
    pub(super) inventory: PlayerInventory,
    pub(super) inventory_operation_revision: u64,
    /// A journal append may have committed. Freeze inventory effects until
    /// restart resolves its after-image; shutdown may save the old watermark.
    pub(super) inventory_recovery_required: bool,
    /// Owner token of the one live compound inventory reservation. While it
    /// is set every other inventory writer of this player refuses; it is
    /// runtime-only and cleared on disconnect.
    pub(super) inventory_reservation: Option<u64>,
    pub(super) carried_item: ItemStack,
    pub(super) crafting_table_input: Option<Box<[ItemStack; 9]>>,
    pub(super) enchanting_table_input: Option<Box<[ItemStack; 2]>>,
    pub(super) merchant_input: Option<Box<[ItemStack; 2]>>,
    pub(super) selected_hotbar_slot: u8,
    pub(super) spawn: SpawnState,
    pub(super) xp: XpState,
    pub(super) inventory_extras: PlayerInventoryExtras,
}

impl PlayerPersistedState {
    pub(super) fn new_default(spawn: PlayerPose) -> Self {
        Self {
            pose: spawn,
            game_mode: GameMode::Survival,
            survival: SurvivalState::FULL,
            effects: None,
            physical_stamina: None,
            physical_hurt_resistance: None,
            physical_shield_disabled_until: None,
            inventory: PlayerInventory::empty(),
            inventory_operation_revision: 0,
            inventory_recovery_required: false,
            inventory_reservation: None,
            carried_item: ItemStack::EMPTY,
            crafting_table_input: None,
            enchanting_table_input: None,
            merchant_input: None,
            selected_hotbar_slot: 0,
            spawn: SpawnState::from_pose(spawn.persisted()),
            xp: XpState::default(),
            inventory_extras: PlayerInventoryExtras::default(),
        }
    }

    /// Inventory writes are refused while a journal outcome is unresolved or
    /// a compound transaction holds the owner reservation.
    pub(super) fn inventory_frozen(&self) -> bool {
        self.inventory_recovery_required || self.inventory_reservation.is_some()
    }

    pub(super) fn replace_inventory(&mut self, inventory: PlayerInventory) {
        if self.inventory.slots != inventory.slots {
            self.inventory = inventory;
        }
    }

    pub(super) fn replace_container(
        &mut self,
        inventory: PlayerInventory,
        carried_item: ItemStack,
    ) {
        if self.inventory.slots != inventory.slots || self.carried_item != carried_item {
            self.inventory = inventory;
            self.carried_item = carried_item;
        }
    }

    pub(super) fn replace_xp(&mut self, xp: XpState) {
        if self.xp != xp {
            self.xp = xp;
        }
    }

    /// The inventory-journal-owned fields of this state with `inventory` in
    /// place of the persisted inventory.
    pub(super) fn owned_inventory<'a>(
        &'a self,
        inventory: &'a PlayerInventory,
    ) -> PlayerOwnedInventory<'a> {
        PlayerOwnedInventory {
            inventory,
            inventory_extras: &self.inventory_extras,
            carried_item: &self.carried_item,
            crafting_table_input: self.crafting_table_input.as_deref(),
            enchanting_table_input: self.enchanting_table_input.as_deref(),
            merchant_input: self.merchant_input.as_deref(),
        }
    }

    fn into_record(self) -> PlayerPersistedRecord {
        PlayerPersistedRecord {
            pose: self.pose.persisted(),
            game_mode: self.game_mode,
            survival: self.survival,
            effects: self.effects.as_ref().map(PlayerEffectsState::snapshot),
            physical_stamina: self.physical_stamina,
            physical_hurt_resistance: self.physical_hurt_resistance,
            physical_shield_disabled_until: self.physical_shield_disabled_until,
            inventory: self.inventory,
            inventory_operation_revision: self.inventory_operation_revision,
            carried_item: self.carried_item,
            crafting_table_input: self.crafting_table_input,
            enchanting_table_input: self.enchanting_table_input,
            merchant_input: self.merchant_input,
            selected_hotbar_slot: self.selected_hotbar_slot,
            spawn: self.spawn,
            xp: self.xp,
            inventory_extras: self.inventory_extras,
        }
    }
}

impl PlayerPose {
    pub(super) fn persisted(self) -> PersistedPlayerPose {
        PersistedPlayerPose {
            x: self.x,
            y: self.y,
            z: self.z,
            yaw: self.yaw,
            pitch: self.pitch,
            on_ground: self.flags.on_ground,
        }
    }

    pub(super) fn from_persisted(persisted: PersistedPlayerPose) -> Self {
        let mut pose = Self::new(persisted.x, persisted.y, persisted.z);
        pose.yaw = persisted.yaw;
        pose.pitch = persisted.pitch;
        pose.flags = MovePlayerFlags::new(persisted.on_ground, false);
        pose
    }
}

pub(super) fn load_player_state(
    world_root: &Path,
    uuid: uuid::Uuid,
    items: &ItemRegistry,
    default: PlayerPersistedState,
) -> Result<Option<PlayerPersistedState>, PlayerPersistenceError> {
    Ok(
        load_player_state_with_pose_loaded(world_root, uuid, items, default)?
            .map(|(state, _pose_loaded)| state),
    )
}

/// Загрузка тем же путём, но с флагом наличия позы в playerdata.
/// `pose_loaded == false` означает, что поза взята из дефолта: вход обязан
/// идти через `resolve_spawn_position`, а не через дешёвый фолбэк.
pub(super) fn load_player_state_with_pose_loaded(
    world_root: &Path,
    uuid: uuid::Uuid,
    items: &ItemRegistry,
    default: PlayerPersistedState,
) -> Result<Option<(PlayerPersistedState, bool)>, PlayerPersistenceError> {
    let default_pose = default.pose;
    let inventory_recovery_required = default.inventory_recovery_required;
    let Some(loaded) = mc_simulation::persistence::load_player_state(
        world_root,
        uuid,
        items,
        default.into_record(),
    )?
    else {
        return Ok(None);
    };
    let pose_loaded = loaded.pose_loaded;
    let record = loaded.record;
    let effects = record
        .effects
        .map(PlayerEffectsState::from_snapshot)
        .transpose()
        .map_err(|_| PlayerPersistenceError::InvalidValue {
            path: playerdata_path(world_root, uuid),
            field: "active_effects",
        })?;
    Ok(Some((
        PlayerPersistedState {
            pose: if pose_loaded {
                PlayerPose::from_persisted(record.pose)
            } else {
                default_pose
            },
            game_mode: record.game_mode,
            survival: record.survival,
            effects,
            physical_stamina: record.physical_stamina,
            physical_hurt_resistance: record.physical_hurt_resistance,
            physical_shield_disabled_until: record.physical_shield_disabled_until,
            inventory: record.inventory,
            inventory_operation_revision: record.inventory_operation_revision,
            inventory_recovery_required,
            inventory_reservation: None,
            carried_item: record.carried_item,
            crafting_table_input: record.crafting_table_input,
            enchanting_table_input: record.enchanting_table_input,
            merchant_input: record.merchant_input,
            selected_hotbar_slot: record.selected_hotbar_slot,
            spawn: record.spawn,
            xp: record.xp,
            inventory_extras: record.inventory_extras,
        },
        pose_loaded,
    )))
}

pub(crate) fn save_player_state(
    world_root: &Path,
    uuid: uuid::Uuid,
    items: &ItemRegistry,
    state: PlayerPersistedState,
) -> Result<(), PlayerPersistenceError> {
    mc_simulation::persistence::save_player_state(world_root, uuid, items, &state.into_record())
}

pub(super) const fn xp_packet(xp: &XpState) -> ClientboundSetExperience {
    ClientboundSetExperience {
        experience_progress: xp.progress,
        total_experience: xp.total,
        experience_level: xp.level,
    }
}

impl fmt::Display for PlayerPersistedState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pos=({:.2},{:.2},{:.2}) mode={:?} health={:.1} food={} selected_slot={}",
            self.pose.x,
            self.pose.y,
            self.pose.z,
            self.game_mode,
            self.survival.health,
            self.survival.food,
            self.selected_hotbar_slot,
        )
    }
}

#[cfg(test)]
#[path = "persistence/effects_tests.rs"]
mod effects_tests;

#[cfg(test)]
#[path = "persistence/inventory_recovery_tests.rs"]
mod inventory_recovery_tests;

#[cfg(test)]
#[path = "persistence_entity_restore_tests.rs"]
mod entity_restore_tests;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn xp_state_adds_points_and_maps_to_wire_packet() {
        let mut xp = XpState::default();

        assert!(!xp.add_points(0));
        assert!(xp.add_points(9));

        assert_eq!(xp.total, 9);
        assert_eq!(xp.level, 1);
        assert!((xp.progress - (2.0 / 9.0)).abs() < f32::EPSILON);
        assert_eq!(
            xp_packet(&xp),
            ClientboundSetExperience {
                experience_progress: xp.progress,
                total_experience: 9,
                experience_level: 1,
            }
        );
    }

    /// Файл playerdata без `Pos`/`Rotation`: тот же кодек, минус поза.
    /// Нужен регрессии входа: такой файл обязан идти через резолв спавна.
    pub(crate) fn write_poseless_playerdata_for_test(world_root: &Path, uuid: uuid::Uuid) {
        use mc_nbt::{ListTag, Tag, tag_type};
        let path = mc_simulation::persistence::playerdata_path(world_root, uuid);
        std::fs::create_dir_all(path.parent().expect("playerdata directory")).unwrap();
        let root = Tag::Compound(vec![
            (
                "Rotation".into(),
                Tag::List(ListTag {
                    element_type: tag_type::FLOAT,
                    elements: vec![Tag::Float(0.0), Tag::Float(0.0)],
                }),
            ),
            ("OnGround".into(), Tag::Byte(0)),
            ("Health".into(), Tag::Float(20.0)),
        ]);
        let mut nbt = Vec::new();
        mc_nbt::write_named(&mut nbt, "", &root).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        use std::io::Write as _;
        encoder.write_all(&nbt).unwrap();
        std::fs::write(path, encoder.finish().unwrap()).unwrap();
    }
}
