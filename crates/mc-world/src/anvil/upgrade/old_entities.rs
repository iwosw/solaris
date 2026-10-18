//! Pre-2586 entity and block-entity fixers reachable within PLAYER records.
//!
//! Bytecode authority: official 26.1.2 `server-26.1.2.jar`
//! (`.analysis/maps/falcons-rock/vanilla-hop/server/versions/26.1.2/server-26.1.2.jar`),
//! registrations in `.analysis/codex-logs/old-dataversion/DataFixers.javap`,
//! per-fixer evidence in `.analysis/codex-logs/old-dataversion/entity-*.javap`,
//! schema evidence in `.analysis/codex-logs/old-dataversion/schema-V*.javap`,
//! version/revision inventory in
//! `.analysis/codex-logs/old-dataversion/legacy-registration-inventory.tsv`.
//! Applies to `Entity` and `BlockEntity` nodes only; the parent wires version
//! order and the tree walk (walk.rs already visits Passengers, shoulder
//! entities, RootVehicle.Entity, and item EntityTag/BlockEntityTag payloads).
//! Player-root UUID/attribute/dimension fields (2514/2523/2537) belong to
//! `old_player_map`; this module handles only nested entity/block-entity
//! payloads. 2514 living-entity attribute UUIDs and 2523 attribute renames reuse
//! `old_player_map::{living_uuid, rename_attributes}` exactly.
use super::{NodeKind, UpgradeError, UpgradeReport, nbt, old_items, old_player_map};
use mc_nbt::Tag;

fn text(value: &str) -> Tag {
    Tag::String(value.into())
}

fn named(value: Option<&Tag>, expected: &str) -> bool {
    value.and_then(nbt::string).is_some_and(|value| {
        value == expected
            || (!value.contains(':') && expected.strip_prefix("minecraft:") == Some(value))
    })
}

/// Migrate one entity or block-entity node at one legacy version.
/// All other node kinds and versions are no-ops; the parent wires version
/// order and the tree walk. Typed BLOCK_STATE upgrades (1450 and the
/// block-rename/property paths) belong to `old_block_states::fix_node`;
/// this module only resolves numeric states via its lookup helpers.
pub(super) fn fix_node(
    tag: &mut Tag,
    kind: NodeKind,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if !matches!(kind, NodeKind::Entity | NodeKind::BlockEntity) {
        return Ok(());
    }
    let entity = kind == NodeKind::Entity;
    // Borrow-safe id access: read the id fresh in each arm; only arms that
    // mutate the id clone it locally (set_id takes &str, no eager alloc).
    match version {
        100 if entity => {
            if equipment(tag) {
                report.applied(version, "EntityEquipmentToArmorAndHandFix");
            }
        }
        101 if entity && nbt::get(tag, "id").and_then(nbt::string) == Some("Villager") => {
            nbt::set(tag, "CanPickUpLoot", Tag::Byte(1));
            report.applied(version, "VillagerSetCanPickUpLootFix");
        }
        106 if !entity && nbt::get(tag, "id").and_then(nbt::string) == Some("MobSpawner") => {
            if spawner_identifiers(tag) {
                report.applied(version, "MobSpawnerEntityIdentifiersFix");
            }
        }
        106 if entity
            && matches!(
                nbt::get(tag, "id").and_then(nbt::string),
                Some("MinecartSpawner") | Some("minecraft:spawner_minecart")
            ) =>
        {
            // UNTAGGED_SPAWNER is nested untyped inside the MinecartSpawner
            // entity (V99 registerEntities template [DisplayTile,
            // UNTAGGED_SPAWNER]); the outer entity id stays untouched, and the
            // pre-705 legacy id is kept until EntityIdFix renames it.
            if spawner_fields(tag) {
                report.applied(version, "MobSpawnerEntityIdentifiersFix");
            }
        }
        107 if entity && nbt::get(tag, "id").and_then(nbt::string) == Some("Minecart") => {
            let kind = match nbt::get(tag, "Type").and_then(nbt::int).unwrap_or(0) {
                1 => "MinecartChest",
                2 => "MinecartFurnace",
                _ => "MinecartRideable",
            };
            set_id(tag, kind);
            nbt::remove(tag, "Type");
            report.applied(version, "EntityMinecartIdentifiersFix");
        }
        108 if entity => {
            if let Some([a, b, c, d]) = nbt::get(tag, "UUID")
                .and_then(nbt::string)
                .and_then(old_items::parse_uuid)
            {
                let most = ((a as i64) << 32) | b as u32 as i64;
                let least = ((c as i64) << 32) | d as u32 as i64;
                nbt::remove(tag, "UUID");
                nbt::set(tag, "UUIDMost", Tag::Long(most));
                nbt::set(tag, "UUIDLeast", Tag::Long(least));
                report.applied(version, "EntityStringUuidFix");
            }
        }
        // ENTITIES set is dead in the jar (declaration + static init only,
        // zero reads); vanilla normalizes HealF/Health on any entity.
        109 if entity => {
            if health(tag) {
                report.applied(version, "EntityHealthFix");
            }
        }
        110 if entity && named(nbt::get(tag, "id"), "EntityHorse") => {
            if horse_saddle(tag) {
                report.applied(version, "EntityHorseSaddleFix");
            }
        }
        111 if entity => {
            if painting_direction(tag) {
                report.applied(version, "EntityPaintingItemFrameDirectionFix");
            }
        }
        113 if entity => {
            if redundant_chances(tag) {
                report.applied(version, "EntityRedundantChanceTagsFix");
            }
        }
        135 if entity => {
            if riding(tag) {
                report.applied(version, "EntityRidingToPassengersFix");
            }
        }
        143 if entity && nbt::get(tag, "id").and_then(nbt::string) == Some("TippedArrow") => {
            set_id(tag, "Arrow");
            report.applied(version, "EntityTippedArrowFix");
        }
        147 if entity && nbt::get(tag, "id").and_then(nbt::string) == Some("ArmorStand") => {
            let silent = nbt::get(tag, "Silent")
                .and_then(nbt::number)
                .is_some_and(|v| v as i8 != 0);
            let marker = nbt::get(tag, "Marker")
                .and_then(nbt::number)
                .is_some_and(|v| v as i8 != 0);
            // Vanilla: remove Silent only when silent && !marker.
            if silent && !marker {
                nbt::remove(tag, "Silent");
                report.applied(version, "EntityArmorStandSilentFix");
            }
        }
        // 165 SignTextStrictJsonFix (NamedEntityFix on BE Sign): Text1..4 via
        // rewriteFromLenient. Lenient/empty/null inputs become EMPTY_CONTENTS
        // ({"text":""}); JSON primitives re-wrap as text components.
        165 if !entity
            && matches!(
                nbt::get(tag, "id").and_then(nbt::string),
                Some("Sign") | Some("minecraft:sign")
            ) =>
        {
            if sign_text(tag) {
                report.applied(version, "SignTextStrictJsonFix");
            }
        }
        502 if entity && nbt::get(tag, "id").and_then(nbt::string) == Some("Zombie") => {
            if zombie_villager_type(tag) {
                report.applied(version, "EntityZombieVillagerTypeFix");
            }
        }
        700 if entity && nbt::get(tag, "id").and_then(nbt::string) == Some("Guardian") => {
            if nbt::get(tag, "Elder")
                .and_then(nbt::number)
                .is_some_and(|v| v as i8 != 0)
            {
                set_id(tag, "ElderGuardian");
                report.applied(version, "EntityElderGuardianSplitFix");
            }
        }
        701 if entity && nbt::get(tag, "id").and_then(nbt::string) == Some("Skeleton") => {
            let renamed = match nbt::get(tag, "SkeletonType")
                .and_then(nbt::int)
                .unwrap_or(0)
            {
                1 => Some("WitherSkeleton"),
                2 => Some("Stray"),
                _ => None,
            };
            // SimpleEntityRenameFix: name changes, remainder tag untouched
            // (SkeletonType retained).
            if let Some(name) = renamed {
                set_id(tag, name);
                report.applied(version, "EntitySkeletonSplitFix");
            }
        }
        702 if entity && nbt::get(tag, "id").and_then(nbt::string) == Some("Zombie") => {
            if zombie_split(tag) {
                report.applied(version, "EntityZombieSplitFix");
            }
        }
        703 if entity && nbt::get(tag, "id").and_then(nbt::string) == Some("EntityHorse") => {
            let name = match nbt::get(tag, "Type").and_then(nbt::int).unwrap_or(0) {
                1 => "Donkey",
                2 => "Mule",
                3 => "ZombieHorse",
                4 => "SkeletonHorse",
                _ => "Horse",
            };
            set_id(tag, name);
            nbt::remove(tag, "Type");
            report.applied(version, "EntityHorseSplitFix");
        }
        704 if !entity => {
            let mapped = nbt::get(tag, "id")
                .and_then(nbt::string)
                .and_then(block_entity_id);
            if let Some(mapped) = mapped {
                set_id(tag, mapped);
                report.applied(version, "BlockEntityIdFix");
            }
        }
        705 if entity => {
            let mapped = nbt::get(tag, "id")
                .and_then(nbt::string)
                .and_then(entity_id);
            if let Some(mapped) = mapped {
                set_id(tag, mapped);
                report.applied(version, "EntityIdFix");
            }
        }
        // 808r1 EntityShulkerColorFix (schema rev 1): runs from version 808.
        808 if entity && named(nbt::get(tag, "id"), "minecraft:shulker") => {
            if nbt::get(tag, "Color").is_none() {
                nbt::set(tag, "Color", Tag::Byte(10));
                report.applied(version, "EntityShulkerColorFix");
            }
        }
        813 if !entity && named(nbt::get(tag, "id"), "minecraft:shulker_box") => {
            if nbt::get(tag, "Color").is_some() {
                nbt::remove(tag, "Color");
                report.applied(version, "BlockEntityShulkerBoxColorFix");
            }
        }
        1451 if !entity && named(nbt::get(tag, "id"), "minecraft:piston") => {
            if piston_block_state(tag) {
                report.applied(version, "BlockEntityBlockStateFix");
            }
        }
        // 1451r3 EntityBlockStateFix and r5 EntityWolfColorFix share one entity
        // arm in registration order so no arm shadows another. VillagerTradeFix
        // (r7) runs ONLY in after_children below: trade ids created by the item
        // flattening (r4) need the post-child pass.
        1451 if entity => {
            if falling_block(tag)
                | minecart_display(tag)
                | enderman_carried(tag)
                | arrow_in_block(tag)
                | remove_throwable_block(tag)
            {
                report.applied(version, "EntityBlockStateFix");
            }
            if named(nbt::get(tag, "id"), "minecraft:wolf")
                && let Some(color) = nbt::get(tag, "CollarColor")
            {
                let mapped = 15 - nbt::int(color).unwrap_or(0);
                nbt::set(tag, "CollarColor", Tag::Byte(mapped as i8));
                report.applied(version, "EntityWolfColorFix");
            }
            // 1451r5 RemoveBlockEntityTagFix, ENTITY branch: falling_block
            // TileEntityData whose nested id is noteblock/flower_pot is
            // dropped (exact class semantics: the TAG's id gates, not the
            // outer entity). Runs after the numeric state rewrites above so
            // the TileEntityData id comparison sees post-1451 names.
            if named(nbt::get(tag, "id"), "minecraft:falling_block")
                && remove_block_entity_data(tag, "TileEntityData")
            {
                report.applied(version, "RemoveBlockEntityTagFix");
            }
        }
        1451 if !entity && named(nbt::get(tag, "id"), "minecraft:jukebox") => {
            if jukebox(tag) {
                report.applied(version, "BlockEntityJukeboxFix");
            }
        }
        1451 if !entity && named(nbt::get(tag, "id"), "minecraft:banner") => {
            if banner_colors(tag) {
                report.applied(version, "BlockEntityBannerColorFix");
            }
        }
        1456 if entity && named(nbt::get(tag, "id"), "minecraft:item_frame") => {
            let facing = nbt::get(tag, "Facing").and_then(nbt::int).unwrap_or(0) as i8;
            nbt::set(tag, "Facing", Tag::Byte(direction_2d_to_3d(facing)));
            report.applied(version, "EntityItemFrameDirectionFix");
        }
        1458 if entity
            && !named(nbt::get(tag, "id"), "minecraft:commandblock_minecart")
            && custom_name_to_component(tag) =>
        {
            report.applied(version, "EntityCustomNameToComponentFix");
        }
        1458 if !entity
            && matches!(
                nbt::get(tag, "id").and_then(nbt::string),
                Some(
                    "minecraft:banner"
                        | "minecraft:beacon"
                        | "minecraft:brewing_stand"
                        | "minecraft:chest"
                        | "minecraft:dispenser"
                        | "minecraft:dropper"
                        | "minecraft:enchanting_table"
                        | "minecraft:furnace"
                        | "minecraft:hopper"
                        | "minecraft:shulker_box"
                        | "minecraft:trapped_chest"
                )
            )
            && custom_name_to_component(tag) =>
        {
            report.applied(version, "BlockEntityCustomNameToComponentFix");
        }
        1460 if entity && named(nbt::get(tag, "id"), "minecraft:painting") => {
            if painting_motive(tag) {
                report.applied(version, "EntityPaintingMotiveFix");
            }
        }
        1474 if entity && named(nbt::get(tag, "id"), "minecraft:shulker") => {
            if colorless_shulker(tag) {
                report.applied(version, "ColorlessShulkerEntityFix");
            }
        }
        // 1488 anonymous DataFixers$1: command_block BE CustomName via
        // fixTagCustomName (same JSON-string conversion, pre-1458 nameables).
        1488 if !entity
            && named(nbt::get(tag, "id"), "minecraft:command_block")
            && custom_name_to_component(tag) =>
        {
            report.applied(version, "CommandBlockCustomNameFix");
        }
        // 1488 anonymous DataFixers$2: commandblock_minecart entity CustomName
        // plain-component conversion (the 1458 exclusion does not apply here).
        1488 if entity
            && named(nbt::get(tag, "id"), "minecraft:commandblock_minecart")
            && custom_name_to_component(tag) =>
        {
            report.applied(version, "CommandBlockMinecartCustomNameFix");
        }
        1483 if entity => {
            if named(nbt::get(tag, "id"), "minecraft:puffer_fish") {
                set_id(tag, "minecraft:pufferfish");
                report.applied(version, "EntityPufferfishRenameFix");
            }
        }
        1486 if entity => {
            if named(nbt::get(tag, "id"), "minecraft:cod_mob") {
                set_id(tag, "minecraft:cod");
                report.applied(version, "EntityCodSalmonFix");
            } else if named(nbt::get(tag, "id"), "minecraft:salmon_mob") {
                set_id(tag, "minecraft:salmon");
                report.applied(version, "EntityCodSalmonFix");
            }
        }
        1500 if !entity && nbt::get(tag, "id").and_then(nbt::string) == Some("DUMMY") => {
            nbt::set(tag, "keepPacked", Tag::Byte(1));
            report.applied(version, "BlockEntityKeepPacked");
        }
        1510 if entity => {
            // Exact rename(): strip minecraft:bred_ then prepend minecraft:,
            // then RENAMED_IDS.getOrDefault (12 pairs, counted from the static
            // block lines 42-76 of entity-EntityTheRenameningFix.javap).
            let current = nbt::get(tag, "id").and_then(nbt::string).unwrap_or("");
            let stripped = current
                .strip_prefix("minecraft:bred_")
                .map(|rest| format!("minecraft:{rest}"));
            let lookup = stripped.as_deref().unwrap_or(current);
            if let Some(mapped) = the_renamening(lookup) {
                set_id(tag, mapped);
                report.applied(version, "EntityTheRenameningFix");
            } else if let Some(stripped) = stripped {
                set_id(tag, &stripped);
                report.applied(version, "EntityTheRenameningFix");
            }
        }
        1904 if entity && named(nbt::get(tag, "id"), "minecraft:ocelot") => {
            if cat_split(tag) {
                report.applied(version, "EntityCatSplitFix");
            }
        }
        1917 if entity && named(nbt::get(tag, "id"), "minecraft:cat") => {
            if nbt::get(tag, "CatType").and_then(nbt::int).unwrap_or(0) == 9 {
                nbt::set(tag, "CatType", Tag::Int(10));
                report.applied(version, "CatTypeFix");
            }
        }
        1918 if entity
            && (named(nbt::get(tag, "id"), "minecraft:villager")
                || named(nbt::get(tag, "id"), "minecraft:zombie_villager")) =>
        {
            if villager_data(tag) {
                report.applied(version, "VillagerDataFix");
            }
        }
        1914 if !entity && named(nbt::get(tag, "id"), "minecraft:chest") => {
            if nbt::get(tag, "LootTable").and_then(nbt::string)
                == Some("minecraft:chests/village_blacksmith")
            {
                nbt::set(
                    tag,
                    "LootTable",
                    text("minecraft:chests/village/village_weaponsmith"),
                );
                report.applied(version, "WeaponSmithChestLootTableFix");
            }
        }
        1928 if entity && named(nbt::get(tag, "id"), "minecraft:illager_beast") => {
            set_id(tag, "minecraft:ravager");
            report.applied(version, "EntityRavagerRenameFix");
        }
        1955 if entity && named(nbt::get(tag, "id"), "minecraft:villager") => {
            if villager_rebuild(tag) {
                report.applied(version, "VillagerRebuildLevelAndXpFix");
            }
        }
        1955 if entity && named(nbt::get(tag, "id"), "minecraft:zombie_villager") => {
            if zombie_villager_rebuild(tag) {
                report.applied(version, "ZombieVillagerRebuildXpFix");
            }
        }
        1953 if !entity && named(nbt::get(tag, "id"), "minecraft:banner") => {
            if ominous_banner(tag) {
                report.applied(version, "OminousBannerBlockEntityRenameFix");
            }
        }
        1963 if entity && named(nbt::get(tag, "id"), "minecraft:villager") => {
            if remove_golem_gossip(tag) {
                report.applied(version, "RemoveGolemGossipFix");
            }
        }
        2501 if !entity
            && matches!(
                nbt::get(tag, "id").and_then(nbt::string),
                Some("minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker")
            ) =>
        {
            if furnace_recipes(tag) {
                report.applied(version, "FurnaceRecipeFix");
            }
        }
        2505 if entity && named(nbt::get(tag, "id"), "minecraft:villager") => {
            if memory_expiry(tag) {
                report.applied(version, "MemoryExpiryDataFix");
            }
        }
        2509 if entity && named(nbt::get(tag, "id"), "minecraft:zombie_pigman") => {
            set_id(tag, "minecraft:zombified_piglin");
            report.applied(version, "EntityZombifiedPiglinRenameFix");
        }
        2511 if entity => {
            if report.from < 2511
                && matches!(
                    nbt::get(tag, "id").and_then(nbt::string),
                    Some(
                        "minecraft:egg"
                            | "minecraft:ender_pearl"
                            | "minecraft:experience_bottle"
                            | "minecraft:snowball"
                            | "minecraft:potion"
                            | "minecraft:llama_spit"
                            | "minecraft:arrow"
                            | "minecraft:spectral_arrow"
                            | "minecraft:trident"
                    )
                )
                && projectile_owner(tag)
            {
                report.applied(version, "EntityProjectileOwnerFix");
            }
            // Revision1 casts the same optional typed value into V2511_1's
            // renamed field. It runs even for integer source2511 and after
            // a successful revision0 owner conversion.
            if named(nbt::get(tag, "id"), "minecraft:potion") && nbt::get(tag, "Potion").is_some() {
                nbt::rename(tag, "Potion", "Item");
                report.applied(version, "SplashPotionItemFieldRenameFix");
            }
        }
        2514 if entity => {
            if entity_uuid(tag) {
                report.applied(version, "EntityUUIDFix");
            }
        }
        2514 if !entity => {
            if block_entity_uuid(tag) {
                report.applied(version, "BlockEntityUUIDFix");
            }
        }
        2516 if entity
            && (named(nbt::get(tag, "id"), "minecraft:villager")
                || named(nbt::get(tag, "id"), "minecraft:zombie_villager"))
            && gossip_uuid(tag) =>
        {
            report.applied(version, "GossipUUIDFix");
        }
        // JigsawPropertiesFix targets the BE root; JigsawRotationFix targets
        // typed BLOCK_STATE and belongs to old_block_states::fix_node.
        2518 if !entity && named(nbt::get(tag, "id"), "minecraft:jigsaw") => {
            if jigsaw_properties(tag) {
                report.applied(version, "JigsawPropertiesFix");
            }
        }
        2523 if entity && old_player_map::rename_attributes(tag) => {
            report.applied(version, "AttributesRenameLegacy");
        }
        2529 if entity && named(nbt::get(tag, "id"), "minecraft:strider") => {
            if nbt::get(tag, "NoGravity")
                .and_then(nbt::number)
                .is_some_and(|v| v as i8 != 0)
            {
                nbt::set(tag, "NoGravity", Tag::Byte(0));
                report.applied(version, "StriderGravityFix");
            }
        }
        2533 if entity && named(nbt::get(tag, "id"), "minecraft:villager") => {
            if villager_follow_range(tag) {
                report.applied(version, "VillagerFollowRangeFix");
            }
        }
        2535 if entity && named(nbt::get(tag, "id"), "minecraft:shulker") => {
            if shulker_rotation(tag) {
                report.applied(version, "EntityShulkerRotationFix");
            }
        }
        _ => {}
    }
    Ok(())
}

/// Post-child hook for the 1451 VillagerTradeFix (r7): runs AFTER the walker's
/// item children (including the r4 flattening) so trade ids created by that
/// flattening are visible. Vanilla targets the minecraft:villager choice.
pub(super) fn after_children(
    tag: &mut Tag,
    kind: NodeKind,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if version == 1451
        && kind == NodeKind::Entity
        && named(nbt::get(tag, "id"), "minecraft:villager")
        && villager_trade(tag)
    {
        report.applied(version, "VillagerTradeFix");
    }
    Ok(())
}

fn set_id(tag: &mut Tag, id: &str) {
    nbt::set(tag, "id", text(id));
}

/// v100 EntityEquipmentToArmorAndHandFix: Equipment[0..4] -> ArmorItems +
/// Equipment[1 spare slot] -> HandItems; DropChances[0] -> HandDropChances,
/// DropChances[1..4] -> ArmorDropChances. Only fills targets that are absent.
/// Elements coerce via asFloat(0) (non-numerics become 0, no slot shifting);
/// a non-list DropChances is stripped with NO default fill.
fn equipment(tag: &mut Tag) -> bool {
    let mut changed = false;
    // Drop chances first (vanilla fixDropChances runs in the same rule).
    if let Some(chances) = nbt::take(tag, "DropChances") {
        changed = true;
        if let Tag::List(list) = chances {
            let mut rest = list
                .elements
                .into_iter()
                .map(|value| nbt::number(&value).unwrap_or(0.0) as f32);
            let first = rest.next().unwrap_or(0.0);
            if nbt::get(tag, "HandDropChances").is_none() {
                nbt::set(
                    tag,
                    "HandDropChances",
                    Tag::List(mc_nbt::ListTag {
                        element_type: mc_nbt::tag_type::FLOAT,
                        elements: vec![Tag::Float(first), Tag::Float(0.0)],
                    }),
                );
                changed = true;
            }
            if nbt::get(tag, "ArmorDropChances").is_none() {
                let armor: Vec<Tag> = rest.map(Tag::Float).collect();
                let mut armor = armor;
                while armor.len() < 4 {
                    armor.push(Tag::Float(0.0));
                }
                armor.truncate(4);
                nbt::set(
                    tag,
                    "ArmorDropChances",
                    Tag::List(mc_nbt::ListTag {
                        element_type: mc_nbt::tag_type::FLOAT,
                        elements: armor,
                    }),
                );
                changed = true;
            }
        }
    }
    let Some(equipment) = nbt::take(tag, "Equipment") else {
        return changed;
    };
    changed = true;
    let items = match equipment {
        Tag::List(list) => list.elements,
        _ => return changed,
    };
    let has_armor = items.len() > 1;
    let mut items = items.into_iter();
    let first = items.next();
    if let Some(first) = first.filter(|_| nbt::get(tag, "HandItems").is_none()) {
        let hand = vec![first, empty_item()];
        nbt::set(
            tag,
            "HandItems",
            Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: hand,
            }),
        );
        changed = true;
    }
    if has_armor && nbt::get(tag, "ArmorItems").is_none() {
        let mut armor: Vec<Tag> = items.take(4).collect();
        while armor.len() < 4 {
            armor.push(empty_item());
        }
        nbt::set(
            tag,
            "ArmorItems",
            Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: armor,
            }),
        );
        changed = true;
    }
    changed
}

fn empty_item() -> Tag {
    nbt::compound(vec![])
}

/// v106 MobSpawnerEntityIdentifiersFix (UNTAGGED_SPAWNER scope): rewrites the
/// untyped spawner remainder in place. EntityId string -> SpawnData{id}
/// (empty EntityId defaults the spawned id to Pig; missing EntityId leaves
/// SpawnData untouched), EntityId removed; SpawnPotentials entries WITH a
/// Type string get {Entity: {id from Type plus preserved Properties fields}},
/// Type/Properties removed, all sibling keys (Weight, etc.) preserved;
/// Type-less entries are left UNCHANGED. Non-string EntityId is kept.
///
/// Scope routing: the block-entity MobSpawner arm stamps the wrapper id
/// MobSpawner (vanilla lambda$makeRule$0 sets id before fix/read); the
/// MinecartSpawner entity arm transforms only the nested spawner fields and
/// MUST NOT touch the outer entity id. Shared field logic lives in
/// spawner_fields so the entity path cannot clobber it.
fn spawner_identifiers(tag: &mut Tag) -> bool {
    // BE path preserves vanilla lambda$makeRule$0: id stamped MobSpawner
    // unconditionally, field rewrite applied on top.
    spawner_fields(tag);
    nbt::set(tag, "id", text("MobSpawner"));
    true
}
/// Returns false when neither EntityId nor SpawnPotentials changed, so the
/// entity-nested MinecartSpawner arm stays a no-op (no report, id kept).
pub(super) fn spawner_fields(tag: &mut Tag) -> bool {
    let mut changed = false;
    if let Some(Tag::String(entity)) = nbt::get_mut(tag, "EntityId") {
        let mut id = std::mem::take(entity);
        if id.is_empty() {
            id.push_str("Pig");
        }
        let mut data = nbt::take(tag, "SpawnData").unwrap_or_else(|| nbt::compound(vec![]));
        if nbt::fields(&data).is_some() {
            nbt::set(&mut data, "id", Tag::String(id));
        }
        nbt::set(tag, "SpawnData", data);
        nbt::remove(tag, "EntityId");
        changed = true;
    }
    if let Some(potentials) = nbt::get_mut(tag, "SpawnPotentials").and_then(nbt::list_mut) {
        for entry in potentials.iter_mut() {
            let Some(Tag::String(kind)) = nbt::get_mut(entry, "Type") else {
                continue;
            };
            let kind = std::mem::take(kind);
            let properties =
                nbt::take(entry, "Properties").unwrap_or_else(|| nbt::compound(vec![]));
            let mut entity = properties;
            if nbt::fields(&entity).is_none() {
                entity = nbt::compound(vec![]);
            }
            nbt::set(&mut entity, "id", Tag::String(kind));
            nbt::remove(entry, "Type");
            nbt::remove(entry, "Properties");
            nbt::set(entry, "Entity", entity);
            changed = true;
        }
    }
    changed
}

/// v109 EntityHealthFix: HealF (else Health) -> Health float, HealF removed.
/// When neither is present the node is untouched.
fn health(tag: &mut Tag) -> bool {
    let healf = nbt::get(tag, "HealF").and_then(nbt::number);
    let health = nbt::get(tag, "Health").and_then(nbt::number);
    let Some(value) = healf.or(health) else {
        return false;
    };
    nbt::remove(tag, "HealF");
    nbt::set(tag, "Health", Tag::Float(value as f32));
    true
}

/// v110 EntityHorseSaddleFix: Saddle=true without SaddleItem ->
/// SaddleItem{minecraft:saddle, Count 1, Damage 0}, Saddle removed.
fn horse_saddle(tag: &mut Tag) -> bool {
    if nbt::get(tag, "SaddleItem").is_some() {
        return false;
    }
    if nbt::get(tag, "Saddle")
        .and_then(nbt::number)
        .is_none_or(|v| v as i8 == 0)
    {
        return false;
    }
    nbt::set(
        tag,
        "SaddleItem",
        nbt::compound(vec![
            ("id", text("minecraft:saddle")),
            ("Count", Tag::Byte(1)),
            ("Damage", Tag::Short(0)),
        ]),
    );
    nbt::remove(tag, "Saddle");
    true
}

fn direction_2d_to_3d(facing: i8) -> i8 {
    match facing {
        0 => 3,
        1 => 4,
        3 => 5,
        _ => 2,
    }
}

/// v111 EntityPaintingItemFrameDirectionFix: the numeric Direction branch
/// shifts tiles and doubles ItemRotation only for item frames. Otherwise the
/// legacy Dir branch supplies Facing without shifting or changing rotation.
fn painting_direction(tag: &mut Tag) -> bool {
    let id = nbt::get(tag, "id").and_then(nbt::string).unwrap_or("");
    if !matches!(id, "Painting" | "ItemFrame")
        || nbt::get(tag, "Facing").and_then(nbt::number).is_some()
    {
        return false;
    }
    let frame = id == "ItemFrame";
    if nbt::get(tag, "Direction").and_then(nbt::number).is_some() {
        shift_tiles(tag);
        if frame && let Some(rotation) = nbt::get(tag, "ItemRotation").and_then(nbt::number) {
            nbt::set(
                tag,
                "ItemRotation",
                Tag::Byte((rotation as i8).wrapping_mul(2)),
            );
        }
    } else {
        let dir = nbt::get(tag, "Dir").and_then(nbt::int).unwrap_or(0) as i8;
        nbt::remove(tag, "Dir");
        nbt::set(tag, "Facing", Tag::Byte(dir % 4));
    }
    true
}

/// Tile shift + Facing from Direction (shared by the painting path and the
/// frame Direction path).
fn shift_tiles(tag: &mut Tag) {
    let dir = nbt::get(tag, "Direction").and_then(nbt::int).unwrap_or(0) as i8;
    let index = dir.rem_euclid(4);
    const OFFSETS: [[i32; 3]; 4] = [[0, 0, 1], [-1, 0, 0], [0, 0, -1], [1, 0, 0]];
    let [dx, dy, dz] = OFFSETS[index as usize];
    for (key, delta) in [("TileX", dx), ("TileY", dy), ("TileZ", dz)] {
        let base = nbt::get(tag, key).and_then(nbt::int).unwrap_or(0);
        nbt::set(tag, key, Tag::Int(base.wrapping_add(delta)));
    }
    nbt::remove(tag, "Direction");
    nbt::set(tag, "Facing", Tag::Byte(index as i8));
}

/// v113 EntityRedundantChanceTagsFix: drop-chance lists that are all zero
/// (HandDropChances len 2, ArmorDropChances len 4) are removed.
fn redundant_chances(tag: &mut Tag) -> bool {
    let mut changed = false;
    for (key, len) in [("HandDropChances", 2), ("ArmorDropChances", 4)] {
        if is_zero_float_list(tag, key, len) {
            nbt::remove(tag, key);
            changed = true;
        }
    }
    changed
}

fn is_zero_float_list(tag: &Tag, key: &str, len: usize) -> bool {
    let Some(list) = nbt::get(tag, key).and_then(nbt::list) else {
        return false;
    };
    list.len() == len && list.iter().all(|v| nbt::number(v).unwrap_or(1.0) == 0.0)
}

/// v135 EntityRidingToPassengersFix: invert each Riding edge in order,
/// carrying the accumulated passenger subtree onto the next vehicle.
fn riding(tag: &mut Tag) -> bool {
    if nbt::get(tag, "Riding").is_none() {
        return false;
    }
    let mut rider = std::mem::replace(tag, nbt::compound(vec![]));
    while let Some(mut vehicle) = nbt::take(&mut rider, "Riding") {
        if nbt::fields(&vehicle).is_none() {
            break;
        }
        nbt::set(
            &mut vehicle,
            "Passengers",
            Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: vec![rider],
            }),
        );
        rider = vehicle;
    }
    *tag = rider;
    true
}

/// v502 EntityZombieVillagerTypeFix: IsVillager + missing ZombieType ->
/// ZombieType from VillagerProfession (0..5 valid; invalid/missing falls back
/// to a deterministic stand-in since vanilla picks nextInt(6) at random and
/// any of the 6 is legitimate), then IsVillager removed. Deterministic port:
/// UUIDMost rem_euclid 6, 0 when no UUID.
fn zombie_villager_type(tag: &mut Tag) -> bool {
    if nbt::get(tag, "IsVillager")
        .and_then(nbt::number)
        .is_none_or(|v| v as i8 == 0)
    {
        return false;
    }
    if nbt::get(tag, "ZombieType").is_some() {
        nbt::remove(tag, "IsVillager");
        return true;
    }
    let profession = nbt::get(tag, "VillagerProfession")
        .and_then(nbt::int)
        .unwrap_or(-1);
    if !(0..6).contains(&profession) {
        // Vanilla picks RandomSource.nextInt(6) at random; any of the 6 is a
        // legitimate vanilla outcome. Deterministic stand-in: stable
        // entity-UUID-derived index (UUIDMost rem_euclid 6; 0 when no UUID).
        let most = nbt::get(tag, "UUIDMost").map_or(0, long);
        nbt::set(tag, "ZombieType", Tag::Int(most.rem_euclid(6) as i32));
        nbt::remove(tag, "IsVillager");
        return true;
    }
    nbt::set(tag, "ZombieType", Tag::Int(profession));
    nbt::remove(tag, "IsVillager");
    true
}

/// v702 EntityZombieSplitFix: Zombie + ZombieType 1..5 -> ZombieVillager with
/// Profession=type-1; 6 -> Husk; else stays Zombie. ZombieType is stripped in
/// ALL arms (vanilla lambda$fix$0 runs on every path).
fn zombie_split(tag: &mut Tag) -> bool {
    if nbt::get(tag, "ZombieType").is_none() {
        return false;
    }
    let zombie_type = nbt::get(tag, "ZombieType").and_then(nbt::int).unwrap_or(0);
    nbt::remove(tag, "ZombieType");
    match zombie_type {
        1..=5 => {
            set_id(tag, "ZombieVillager");
            nbt::set(tag, "Profession", Tag::Int(zombie_type - 1));
        }
        6 => {
            set_id(tag, "Husk");
        }
        _ => {}
    }
    true
}

fn block_entity_id(id: &str) -> Option<&'static str> {
    Some(match id {
        "Airportal" => "minecraft:end_portal",
        "Banner" => "minecraft:banner",
        "Beacon" => "minecraft:beacon",
        "Cauldron" => "minecraft:brewing_stand",
        "Chest" => "minecraft:chest",
        "Comparator" => "minecraft:comparator",
        "Control" => "minecraft:command_block",
        "DLDetector" => "minecraft:daylight_detector",
        "Dropper" => "minecraft:dropper",
        "EnchantTable" => "minecraft:enchanting_table",
        "EndGateway" => "minecraft:end_gateway",
        "EnderChest" => "minecraft:ender_chest",
        "FlowerPot" => "minecraft:flower_pot",
        "Furnace" => "minecraft:furnace",
        "Hopper" => "minecraft:hopper",
        "MobSpawner" => "minecraft:mob_spawner",
        "Music" => "minecraft:noteblock",
        "Piston" => "minecraft:piston",
        "RecordPlayer" => "minecraft:jukebox",
        "Sign" => "minecraft:sign",
        "Skull" => "minecraft:skull",
        "Structure" => "minecraft:structure_block",
        "Trap" => "minecraft:dispenser",
        _ => return None,
    })
}

fn entity_id(id: &str) -> Option<&'static str> {
    Some(match id {
        "AreaEffectCloud" => "minecraft:area_effect_cloud",
        "ArmorStand" => "minecraft:armor_stand",
        "Arrow" => "minecraft:arrow",
        "Bat" => "minecraft:bat",
        "Blaze" => "minecraft:blaze",
        "Boat" => "minecraft:boat",
        "CaveSpider" => "minecraft:cave_spider",
        "Chicken" => "minecraft:chicken",
        "Cow" => "minecraft:cow",
        "Creeper" => "minecraft:creeper",
        "Donkey" => "minecraft:donkey",
        "DragonFireball" => "minecraft:dragon_fireball",
        "ElderGuardian" => "minecraft:elder_guardian",
        "EnderCrystal" => "minecraft:ender_crystal",
        "EnderDragon" => "minecraft:ender_dragon",
        "Enderman" => "minecraft:enderman",
        "Endermite" => "minecraft:endermite",
        "EyeOfEnderSignal" => "minecraft:eye_of_ender_signal",
        "FallingSand" => "minecraft:falling_block",
        "Fireball" => "minecraft:fireball",
        "FireworksRocketEntity" => "minecraft:fireworks_rocket",
        "Ghast" => "minecraft:ghast",
        "Giant" => "minecraft:giant",
        "Guardian" => "minecraft:guardian",
        "Horse" => "minecraft:horse",
        "Husk" => "minecraft:husk",
        "Item" => "minecraft:item",
        "ItemFrame" => "minecraft:item_frame",
        "LavaSlime" => "minecraft:magma_cube",
        "LeashKnot" => "minecraft:leash_knot",
        "MinecartChest" => "minecraft:chest_minecart",
        "MinecartCommandBlock" => "minecraft:commandblock_minecart",
        "MinecartFurnace" => "minecraft:furnace_minecart",
        "MinecartHopper" => "minecraft:hopper_minecart",
        "MinecartRideable" => "minecraft:minecart",
        "MinecartSpawner" => "minecraft:spawner_minecart",
        "MinecartTNT" => "minecraft:tnt_minecart",
        "Mule" => "minecraft:mule",
        "MushroomCow" => "minecraft:mooshroom",
        "Ozelot" => "minecraft:ocelot",
        "Painting" => "minecraft:painting",
        "Pig" => "minecraft:pig",
        "PigZombie" => "minecraft:zombie_pigman",
        "PolarBear" => "minecraft:polar_bear",
        "PrimedTnt" => "minecraft:tnt",
        "Rabbit" => "minecraft:rabbit",
        "Sheep" => "minecraft:sheep",
        "Shulker" => "minecraft:shulker",
        "ShulkerBullet" => "minecraft:shulker_bullet",
        "Silverfish" => "minecraft:silverfish",
        "Skeleton" => "minecraft:skeleton",
        "SkeletonHorse" => "minecraft:skeleton_horse",
        "Slime" => "minecraft:slime",
        "SmallFireball" => "minecraft:small_fireball",
        "SnowMan" => "minecraft:snowman",
        "Snowball" => "minecraft:snowball",
        "SpectralArrow" => "minecraft:spectral_arrow",
        "Spider" => "minecraft:spider",
        "Squid" => "minecraft:squid",
        "Stray" => "minecraft:stray",
        "ThrownEgg" => "minecraft:egg",
        "ThrownEnderpearl" => "minecraft:ender_pearl",
        "ThrownExpBottle" => "minecraft:xp_bottle",
        "ThrownPotion" => "minecraft:potion",
        "Villager" => "minecraft:villager",
        "VillagerGolem" => "minecraft:villager_golem",
        "Witch" => "minecraft:witch",
        "WitherBoss" => "minecraft:wither",
        "WitherSkeleton" => "minecraft:wither_skeleton",
        "WitherSkull" => "minecraft:wither_skull",
        "Wolf" => "minecraft:wolf",
        "XPOrb" => "minecraft:xp_orb",
        "Zombie" => "minecraft:zombie",
        "ZombieHorse" => "minecraft:zombie_horse",
        "ZombieVillager" => "minecraft:zombie_villager",
        _ => return None,
    })
}

/// v1451.2 BlockEntityBlockStateFix (piston only): blockId/blockData ->
/// blockState via BlockStateData.getTag(id<<4 | data&15); fields removed.
/// `state()` returns MAP[0] (air) for invalid ids, exactly like vanilla.
fn piston_block_state(tag: &mut Tag) -> bool {
    let id = nbt::get(tag, "blockId").and_then(nbt::int).unwrap_or(0);
    let data = nbt::get(tag, "blockData").and_then(nbt::int).unwrap_or(0) & 15;
    nbt::remove(tag, "blockId");
    nbt::remove(tag, "blockData");
    nbt::set(
        tag,
        "blockState",
        super::old_block_states::state(id << 4 | data),
    );
    true
}

/// v1451.3 EntityBlockStateFix falling block: TileID/Data/Tile + Block/Data
/// -> BlockState via BlockStateData.getTag; Data/TileID/Tile removed.
/// Numeric BlockStateData ids are POST-flattening: never run `upgrade` on the
/// freshly resolved state.
fn falling_block(tag: &mut Tag) -> bool {
    if !named(nbt::get(tag, "id"), "minecraft:falling_block") {
        return false;
    }
    update_falling_block(tag)
}

/// Exact updateFallingBlock: prefer typed Block, otherwise numeric TileID,
/// otherwise the unsigned Tile byte; all three forms share the Data nibble.
fn update_falling_block(tag: &mut Tag) -> bool {
    let data = nbt::get(tag, "Data").and_then(nbt::int).unwrap_or(0) & 15;
    let id = if let Some(block) = nbt::take(tag, "Block") {
        match block {
            Tag::String(name) => super::old_block_states::block_id(&name),
            other => nbt::int(&other).unwrap_or(0),
        }
    } else {
        nbt::get(tag, "TileID")
            .and_then(nbt::number)
            .map(|value| value as i32)
            .unwrap_or_else(|| {
                nbt::get(tag, "Tile").and_then(nbt::number).unwrap_or(0.0) as i32 & 255
            })
    };
    for key in ["Data", "TileID", "Tile"] {
        nbt::remove(tag, key);
    }
    nbt::set(
        tag,
        "BlockState",
        super::old_block_states::state(id << 4 | data),
    );
    true
}

fn minecart_display(tag: &mut Tag) -> bool {
    if !matches!(
        nbt::get(tag, "id").and_then(nbt::string),
        Some(
            "minecraft:minecart"
                | "minecraft:chest_minecart"
                | "minecraft:tnt_minecart"
                | "minecraft:spawner_minecart"
                | "minecraft:hopper_minecart"
                | "minecraft:furnace_minecart"
                | "minecraft:commandblock_minecart"
        )
    ) {
        return false;
    }
    block_to_state(tag, "DisplayTile", "DisplayData", "DisplayState")
}

fn enderman_carried(tag: &mut Tag) -> bool {
    if !named(nbt::get(tag, "id"), "minecraft:enderman") {
        return false;
    }
    block_to_state(tag, "carried", "carriedData", "carriedBlockState")
}

fn arrow_in_block(tag: &mut Tag) -> bool {
    if !matches!(
        nbt::get(tag, "id").and_then(nbt::string),
        Some("minecraft:arrow" | "minecraft:spectral_arrow")
    ) {
        return false;
    }
    block_to_state(tag, "inTile", "inData", "inBlockState")
}

fn remove_throwable_block(tag: &mut Tag) -> bool {
    matches!(
        nbt::get(tag, "id").and_then(nbt::string),
        Some(
            "minecraft:egg"
                | "minecraft:ender_pearl"
                | "minecraft:fireball"
                | "minecraft:potion"
                | "minecraft:small_fireball"
                | "minecraft:snowball"
                | "minecraft:wither_skull"
                | "minecraft:xp_bottle"
        )
    ) && nbt::take(tag, "inTile").is_some()
}

fn block_to_state(tag: &mut Tag, block_key: &str, data_key: &str, state_key: &str) -> bool {
    let Some(block) = nbt::take(tag, block_key) else {
        return nbt::take(tag, data_key).is_some();
    };
    let data = nbt::get(tag, data_key).and_then(nbt::int).unwrap_or(0) & 15;
    let id = match block {
        Tag::String(name) => super::old_block_states::block_id(&name),
        other => nbt::int(&other).unwrap_or(0),
    };
    nbt::remove(tag, data_key);
    nbt::set(
        tag,
        state_key,
        super::old_block_states::state(id << 4 | data),
    );
    true
}
/// v1451.6 BlockEntityJukeboxFix: convert positive Record through the full
/// ItemIdFix/flattening tables and overwrite RecordItem only on a match.
/// Vanilla discards Dynamic.remove(Record)'s return value, preserving Record.
fn jukebox(tag: &mut Tag) -> bool {
    let record = nbt::get(tag, "Record").and_then(nbt::int).unwrap_or(0);
    if record <= 0 {
        return false;
    }
    let Some(flat) = old_items::flattened_item(old_items::numeric_item(record), 0) else {
        return false;
    };
    nbt::set(
        tag,
        "RecordItem",
        nbt::compound(vec![("id", text(flat)), ("Count", Tag::Byte(1))]),
    );
    true
}

/// v1451 BlockEntityBannerColorFix: Base/Patterns[].Color 15-x -> int color.
fn banner_colors(tag: &mut Tag) -> bool {
    let mut changed = false;
    if let Some(base) = nbt::get(tag, "Base").and_then(nbt::int) {
        nbt::set(tag, "Base", Tag::Int(15 - base));
        changed = true;
    }
    if let Some(patterns) = nbt::get_mut(tag, "Patterns").and_then(nbt::list_mut) {
        for pattern in patterns.iter_mut() {
            if let Some(color) = nbt::get(pattern, "Color").and_then(nbt::int) {
                nbt::set(pattern, "Color", Tag::Int(15 - color));
                changed = true;
            }
        }
    }
    changed
}

/// v1451 VillagerTradeFix: carved_pumpkin -> pumpkin in trade item ids.
fn villager_trade(tag: &mut Tag) -> bool {
    let mut changed = false;
    if let Some(offers) = nbt::get_mut(tag, "Offers") {
        let recipes = nbt::get_mut(offers, "Recipes").and_then(nbt::list_mut);
        if let Some(recipes) = recipes {
            for recipe in recipes.iter_mut() {
                for key in ["buy", "buyB", "sell"] {
                    if let Some(item) = nbt::get_mut(recipe, key)
                        && nbt::get(item, "id").and_then(nbt::string)
                            == Some("minecraft:carved_pumpkin")
                    {
                        nbt::set(item, "id", text("minecraft:pumpkin"));
                        changed = true;
                    }
                }
            }
        }
    }
    changed
}

/// v165 SignTextStrictJsonFix (BE Sign): Text1..4 through
/// rewriteFromLenient. Empty/"null" inputs become EMPTY_CONTENTS
/// (createTextComponentJson("") = {"text":""}); quoted/braced/bracketed
/// inputs parse as lenient JSON (primitives re-wrap as text, compounds
/// stabilize); unparseable inputs wrap as plain text.
fn sign_text(tag: &mut Tag) -> bool {
    let mut changed = false;
    for key in ["Text1", "Text2", "Text3", "Text4"] {
        if rewrite_lenient(tag, key) {
            changed = true;
        }
    }
    changed
}

fn rewrite_lenient(tag: &mut Tag, key: &str) -> bool {
    let Some(Tag::String(current)) = nbt::get_mut(tag, key) else {
        return false;
    };
    *current = old_items::rewrite_from_lenient(current);
    true
}

/// v1953 OminousBannerBlockEntityRenameFix (banner BE): CustomName text
/// component with translate block.minecraft.illager_banner -> ominous_banner.
fn ominous_banner(tag: &mut Tag) -> bool {
    let Some(name) = nbt::get(tag, "CustomName")
        .and_then(nbt::string)
        .map(str::to_owned)
    else {
        return false;
    };
    if !name.contains("block.minecraft.illager_banner") {
        return false;
    }
    nbt::set(
        tag,
        "CustomName",
        Tag::String(name.replace(
            "block.minecraft.illager_banner",
            "block.minecraft.ominous_banner",
        )),
    );
    true
}
/// v1458 plain-text CustomName -> JSON string {"text":"name"}; empty names
/// removed. (LegacyComponentDataFixUtils.createPlainTextComponent output is a
/// STRING holding JSON, which the 4290 text chain parses.)
fn custom_name_to_component(tag: &mut Tag) -> bool {
    let Some(Tag::String(name)) = nbt::get_mut(tag, "CustomName") else {
        return false;
    };
    if name.is_empty() {
        nbt::remove(tag, "CustomName");
        return true;
    }
    *name = old_items::create_text_component_json(name);
    true
}

/// v1460 EntityPaintingMotiveFix: Motive lowercased, three legacy spellings
/// remapped, then namespaced.
fn painting_motive(tag: &mut Tag) -> bool {
    let Some(motive) = nbt::get(tag, "Motive")
        .and_then(nbt::string)
        .map(str::to_owned)
    else {
        return false;
    };
    let lower = motive.to_ascii_lowercase();
    let mapped = match lower.as_str() {
        "donkeykong" => "donkey_kong",
        "burningskull" => "burning_skull",
        "skullandroses" => "skull_and_roses",
        _ => lower.as_str(),
    };
    let namespaced = if mapped.contains(':') {
        mapped.into()
    } else {
        format!("minecraft:{mapped}")
    };
    nbt::set(tag, "Motive", text(&namespaced));
    true
}

/// v1474 ColorlessShulkerEntityFix: Color 10 -> 16.
fn colorless_shulker(tag: &mut Tag) -> bool {
    if nbt::get(tag, "Color").and_then(nbt::int).unwrap_or(-1) == 10 {
        nbt::set(tag, "Color", Tag::Byte(16));
        return true;
    }
    false
}

/// v1904 EntityCatSplitFix (SimpleEntityRenameFix on minecraft:ocelot):
/// - CatType==0 with Owner/OwnerUUID -> stays ocelot, Trusting=true;
/// - 1..=3 -> minecraft:cat (CatType kept, OwnerUUID string round-trip);
/// - else stays ocelot unchanged.
fn cat_split(tag: &mut Tag) -> bool {
    let cat_type = nbt::get(tag, "CatType").and_then(nbt::int).unwrap_or(0);
    if cat_type == 0 {
        let owned = nbt::get(tag, "Owner")
            .and_then(nbt::string)
            .is_some_and(|o| !o.is_empty())
            || nbt::get(tag, "OwnerUUID")
                .and_then(nbt::string)
                .is_some_and(|o| !o.is_empty());
        if owned {
            nbt::set(tag, "Trusting", Tag::Byte(1));
            return true;
        }
        return false;
    }
    if (1..4).contains(&cat_type) {
        set_id(tag, "minecraft:cat");
        return true;
    }
    false
}

/// v1918 VillagerDataFix: Profession/Career/CareerLevel ->
/// VillagerData{type: minecraft:plains, profession: upgradeData, level:
/// CareerLevel tag when present else Int(1)}; only the three legacy keys are
/// removed (root `type` remainder, if any, is kept).
fn villager_data(tag: &mut Tag) -> bool {
    let profession = nbt::get(tag, "Profession").and_then(nbt::int).unwrap_or(0);
    let career = nbt::get(tag, "Career").and_then(nbt::int).unwrap_or(0);
    let level = nbt::get(tag, "CareerLevel").cloned().unwrap_or(Tag::Int(1));
    nbt::remove(tag, "Profession");
    nbt::remove(tag, "Career");
    nbt::remove(tag, "CareerLevel");
    nbt::set(
        tag,
        "VillagerData",
        nbt::compound(vec![
            ("type", text("minecraft:plains")),
            ("profession", text(upgrade_profession(profession, career))),
            ("level", level),
        ]),
    );
    true
}

fn upgrade_profession(profession: i32, career: i32) -> &'static str {
    match (profession, career) {
        (0, 2) => "minecraft:fisherman",
        (0, 3) => "minecraft:shepherd",
        (0, 4) => "minecraft:fletcher",
        (0, _) => "minecraft:farmer",
        (1, 2) => "minecraft:cartographer",
        (1, _) => "minecraft:librarian",
        (2, _) => "minecraft:cleric",
        (3, 2) => "minecraft:weaponsmith",
        (3, 3) => "minecraft:toolsmith",
        (3, _) => "minecraft:armorer",
        (4, 2) => "minecraft:leatherworker",
        (4, _) => "minecraft:butcher",
        (5, _) => "minecraft:nitwit",
        _ => "minecraft:none",
    }
}

/// v1955 VillagerRebuildLevelAndXpFix: rebuild novice levels from trades,
/// then supply missing/non-numeric Xp for every level.
fn villager_rebuild(tag: &mut Tag) -> bool {
    let mut level = nbt::get(tag, "VillagerData")
        .and_then(|data| nbt::get(data, "level"))
        .and_then(nbt::int)
        .unwrap_or(0);
    let mut changed = false;
    if level == 0 || level == 1 {
        let count = nbt::get(tag, "Offers")
            .and_then(|offers| nbt::get(offers, "Recipes"))
            .and_then(nbt::list)
            .map_or(0, |recipes| recipes.len());
        level = (count as i32 / 2).clamp(1, 5);
        if level > 1
            && let Some(data) = nbt::get_mut(tag, "VillagerData")
        {
            nbt::set(data, "level", Tag::Int(level));
            changed = true;
        }
    }
    if nbt::get(tag, "Xp").and_then(nbt::number).is_none() {
        const THRESHOLDS: [i32; 5] = [0, 10, 50, 100, 150];
        let xp = THRESHOLDS[(level - 1).clamp(0, 4) as usize];
        nbt::set(tag, "Xp", Tag::Int(xp));
        changed = true;
    }
    changed
}

/// v1955 ZombieVillagerRebuildXpFix: missing Xp -> thresholds[VillagerData.level].
fn zombie_villager_rebuild(tag: &mut Tag) -> bool {
    if nbt::get(tag, "Xp").and_then(nbt::number).is_some() {
        return false;
    }
    let level = nbt::get(tag, "VillagerData")
        .and_then(|data| nbt::get(data, "level"))
        .and_then(nbt::int)
        .unwrap_or(1);
    const THRESHOLDS: [i32; 5] = [0, 10, 50, 100, 150];
    nbt::set(
        tag,
        "Xp",
        Tag::Int(THRESHOLDS[(level - 1).clamp(0, 4) as usize]),
    );
    true
}

/// v1963 RemoveGolemGossipFix: drop Gossips entries with Type=="golem".
fn remove_golem_gossip(tag: &mut Tag) -> bool {
    let Some(gossips) = nbt::get_mut(tag, "Gossips").and_then(nbt::list_mut) else {
        return false;
    };
    let before = gossips.len();
    gossips.retain(|entry| nbt::get(entry, "Type").and_then(nbt::string) != Some("golem"));
    before != gossips.len()
}

/// v1451r5 RemoveBlockEntityTagFix, ENTITY falling_block branch: drop
/// TileEntityData (key `field`) when its nested `id` is minecraft:noteblock
/// or minecraft:flower_pot. The nested tag's id gates removal; by this version
/// its pre704 discriminator has already been upgraded. The structure-block
/// branch is not PLAYER-reachable.
fn remove_block_entity_data(tag: &mut Tag, field: &str) -> bool {
    let drop = matches!(
        nbt::get(tag, field)
            .and_then(|data| nbt::get(data, "id"))
            .and_then(nbt::string),
        Some("minecraft:noteblock" | "minecraft:flower_pot")
    );
    if !drop {
        return false;
    }
    nbt::remove(tag, field);
    true
}

/// v2501 FurnaceRecipeFix (furnace/blast_furnace/smoker BEs):
/// RecipesUsedSize N + RecipeLocation{i}/RecipeAmount{i} (i in 0..N) fold
/// into RecipesUsed{location: amount}; RecipesUsedSize removed. Only
/// positive amounts with a present location string are kept
/// (vanilla reads the amount with default 0 and skips <= 0, and skips
/// missing locations via Optional.ifPresent). Indexed keys are removed
/// unconditionally once consumed (vanilla removes both keys per index even
/// when the entry is skipped). RECIPE-typed conversion is schema-level
/// (no payload shape change here).
fn furnace_recipes(tag: &mut Tag) -> bool {
    // Vanilla updateFurnaceContents runs on EVERY furnace/blast_furnace/smoker
    // BE at 2501 (fixTypeEverywhereTyped over the three named choices): the
    // size key is always removed and RecipesUsed always set, even when empty.
    let size = nbt::get(tag, "RecipesUsedSize")
        .and_then(nbt::int)
        .unwrap_or(0);
    nbt::remove(tag, "RecipesUsedSize");
    let mut used: Vec<(String, Tag)> = Vec::new();
    for i in 0..size.max(0) {
        let location_key = format!("RecipeLocation{i}");
        let amount_key = format!("RecipeAmount{i}");
        let location = nbt::get(tag, &location_key)
            .and_then(nbt::string)
            .map(str::to_owned);
        let amount = nbt::get(tag, &amount_key).and_then(nbt::int).unwrap_or(0);
        nbt::remove(tag, &location_key);
        nbt::remove(tag, &amount_key);
        if amount <= 0 {
            continue;
        }
        if let Some(location) = location {
            used.push((location, Tag::Int(amount)));
        }
    }
    nbt::set(tag, "RecipesUsed", Tag::Compound(used));
    true
}
/// v2505 MemoryExpiryDataFix (minecraft:villager): wrap EVERY
/// Brain.memories value as {value: previous}, unconditionally.
fn memory_expiry(tag: &mut Tag) -> bool {
    let Some(brain) = nbt::get_mut(tag, "Brain") else {
        return false;
    };
    let Some(memories) = nbt::get_mut(brain, "memories") else {
        return false;
    };
    let Some(fields) = nbt::fields_mut(memories) else {
        return false;
    };
    for (_, value) in fields.iter_mut() {
        let old = std::mem::replace(value, nbt::compound(vec![]));
        *value = nbt::compound(vec![("value", old)]);
    }
    true
}

/// v2511 EntityProjectileOwnerFix, three disjoint shapes:
/// - arrows (arrow/spectral_arrow/trident): top-level OwnerUUIDMost/Least ->
///   OwnerUUID int-array (zero-guard), both keys removed;
/// - llama_spit only: nested Owner{OwnerUUIDMost,OwnerUUIDLeast} -> parent
///   OwnerUUID int-array when both non-zero; Owner ALWAYS removed;
/// - throwables (egg/ender_pearl/xp_bottle/snowball/potion): lowercase
///   owner{M,L} -> OwnerUUID when both non-zero; owner ALWAYS removed.
fn projectile_owner(tag: &mut Tag) -> bool {
    let id = nbt::get(tag, "id").and_then(nbt::string).unwrap_or("");
    if id == "minecraft:arrow" || id == "minecraft:spectral_arrow" || id == "minecraft:trident" {
        return old_player_map::replace_uuid(tag, "OwnerUUID", "OwnerUUID");
    }
    if id == "minecraft:llama_spit" {
        if let Some(owner) = nbt::take(tag, "Owner") {
            return llama_owner(tag, owner);
        }
        return false;
    }
    if let Some(owner) = nbt::take(tag, "owner") {
        return throwable_owner(tag, owner);
    }
    false
}

/// Llama-spit nested owner conversion (called with the taken Owner compound).
fn llama_owner(parent: &mut Tag, owner: Tag) -> bool {
    let most = nbt::get(&owner, "OwnerUUIDMost").map_or(0, long);
    let least = nbt::get(&owner, "OwnerUUIDLeast").map_or(0, long);
    if most != 0 && least != 0 {
        set_uuid_array(parent, "OwnerUUID", most, least);
    }
    true
}

/// Throwable lowercase owner conversion (called with the taken owner compound).
fn throwable_owner(parent: &mut Tag, owner: Tag) -> bool {
    let (most, least) = match &owner {
        Tag::Compound(fields) => {
            let most = fields
                .iter()
                .find(|(k, _)| k == "M")
                .map_or(0, |(_, value)| long(value));
            let least = fields
                .iter()
                .find(|(k, _)| k == "L")
                .map_or(0, |(_, value)| long(value));
            (most, least)
        }
        _ => (0, 0),
    };
    if most != 0 && least != 0 {
        set_uuid_array(parent, "OwnerUUID", most, least);
    }
    true
}

fn set_uuid_array(tag: &mut Tag, key: &str, most: i64, least: i64) {
    nbt::set(
        tag,
        key,
        Tag::IntArray(vec![
            (most >> 32) as i32,
            most as i32,
            (least >> 32) as i32,
            least as i32,
        ]),
    );
}

/// Read Number.longValue without rounding the low bits of an NBT long via f64.
fn long(value: &Tag) -> i64 {
    match value {
        Tag::Long(value) => *value,
        value => nbt::number(value).unwrap_or(0.0) as i64,
    }
}

/// v2514 EntityUUIDFix: each family inherits its living/mob/animal rewrites;
/// only horses and tameables have the OwnerUUID string reference.
fn entity_uuid(tag: &mut Tag) -> bool {
    let id = nbt::get(tag, "id").and_then(nbt::string).unwrap_or("");
    let (horse, tameable, animal, mob, projectile) = (
        is_horse(id),
        is_tameable(id),
        is_animal(id),
        is_mob(id),
        is_projectile(id),
    );
    let living = id == "minecraft:armor_stand" || mob;
    let hurt = matches!(id, "minecraft:bee" | "minecraft:zombified_piglin");
    let special: u8 = match id {
        "minecraft:fox" => 0,
        "minecraft:item" => 1,
        "minecraft:shulker_bullet" => 2,
        "minecraft:area_effect_cloud" => 3,
        "minecraft:zombie_villager" => 4,
        "minecraft:evoker_fangs" => 5,
        "minecraft:piglin" => 6,
        _ => 7,
    };
    let mut changed = old_player_map::replace_uuid(tag, "UUID", "UUID");
    if living {
        changed |= old_player_map::living_uuid(tag);
    }
    if horse || tameable {
        changed |= replace_uuid_string(tag, "OwnerUUID", "Owner");
    }
    if animal {
        changed |= old_player_map::replace_uuid(tag, "LoveCause", "LoveCause");
    }
    if mob {
        changed |= replace_leash_uuid(tag);
    }
    if projectile {
        changed |= move_uuid_array(tag, "OwnerUUID", "Owner");
    }
    if special == 0 {
        changed |= fox_trusted(tag);
    }
    if special == 1 {
        changed |= replace_uuid_ml(tag, "Owner", "Owner");
        changed |= replace_uuid_ml(tag, "Thrower", "Thrower");
    }
    if special == 2 {
        changed |= replace_uuid_ml(tag, "Owner", "Owner");
        changed |= replace_uuid_ml(tag, "Target", "Target");
    }
    if special == 3 {
        changed |= old_player_map::replace_uuid(tag, "OwnerUUID", "Owner");
    }
    if special == 4 {
        changed |= old_player_map::replace_uuid(tag, "ConversionPlayer", "ConversionPlayer");
    }
    if special == 5 {
        changed |= old_player_map::replace_uuid(tag, "OwnerUUID", "Owner");
    }
    if special == 6 {
        changed |= piglin_angry_at(tag);
    }
    if hurt {
        changed |= replace_uuid_string(tag, "HurtBy", "HurtBy");
    }
    changed
}

/// AbstractUUIDFix.replaceUUIDMLTag: X{M,L} compound -> Target int-array
/// (both halves non-zero or no-op; source key removed on success).
fn replace_uuid_ml(tag: &mut Tag, source: &str, target: &str) -> bool {
    let (most, least) = match nbt::get(tag, source) {
        Some(Tag::Compound(fields)) => {
            let most = fields
                .iter()
                .find(|(k, _)| k == "M")
                .map_or(0, |(_, value)| long(value));
            let least = fields
                .iter()
                .find(|(k, _)| k == "L")
                .map_or(0, |(_, value)| long(value));
            (most, least)
        }
        _ => return false,
    };
    if most == 0 || least == 0 {
        return false;
    }
    nbt::remove(tag, source);
    set_uuid_array(tag, target, most, least);
    true
}

/// AbstractUUIDFix.replaceUUIDString uses Java UUID.fromString, including its
/// short-group fallback and nil UUID (unlike the nonzero-half ML helper).
fn replace_uuid_string(tag: &mut Tag, source: &str, target: &str) -> bool {
    let Some(uuid) = nbt::get(tag, source)
        .and_then(nbt::string)
        .and_then(old_items::parse_uuid)
    else {
        return false;
    };
    nbt::remove(tag, source);
    nbt::set(tag, target, Tag::IntArray(uuid.to_vec()));
    true
}

/// updateProjectile moves any existing OwnerUUID value, overwriting Owner.
fn move_uuid_array(tag: &mut Tag, source: &str, target: &str) -> bool {
    let Some(value) = nbt::take(tag, source) else {
        return false;
    };
    nbt::set(tag, target, value);
    true
}

fn replace_leash_uuid(tag: &mut Tag) -> bool {
    let Some(leash) = nbt::get_mut(tag, "Leash") else {
        return false;
    };
    old_player_map::replace_uuid(leash, "UUID", "UUID")
}

fn fox_trusted(tag: &mut Tag) -> bool {
    let Some(list) = nbt::take(tag, "TrustedUUIDs") else {
        return false;
    };
    let elements = match list {
        Tag::List(list) => list.elements,
        other => {
            nbt::set(tag, "TrustedUUIDs", other);
            return false;
        }
    };
    // Vanilla keeps unconvertible entries (orElseGet original), never drops.
    let mut trusted = Vec::with_capacity(elements.len());
    for entry in elements {
        let pair = match &entry {
            Tag::Compound(fields) => {
                let most = fields
                    .iter()
                    .find(|(k, _)| k == "M")
                    .map_or(0, |(_, value)| long(value));
                let least = fields
                    .iter()
                    .find(|(k, _)| k == "L")
                    .map_or(0, |(_, value)| long(value));
                (most != 0 && least != 0).then(|| {
                    Tag::IntArray(vec![
                        (most >> 32) as i32,
                        most as i32,
                        (least >> 32) as i32,
                        least as i32,
                    ])
                })
            }
            _ => None,
        };
        trusted.push(pair.unwrap_or(entry));
    }
    nbt::set(
        tag,
        "Trusted",
        Tag::List(mc_nbt::ListTag {
            element_type: mc_nbt::tag_type::INT_ARRAY,
            elements: trusted,
        }),
    );
    true
}

fn piglin_angry_at(tag: &mut Tag) -> bool {
    let Some(brain) = nbt::get_mut(tag, "Brain") else {
        return false;
    };
    let Some(memories) = nbt::get_mut(brain, "memories") else {
        return false;
    };
    let Some(angry) = nbt::get_mut(memories, "minecraft:angry_at") else {
        return false;
    };
    let Some(value) = nbt::get_mut(angry, "value") else {
        return false;
    };
    let Some(uuid) = nbt::string(value).and_then(old_items::parse_uuid) else {
        return false;
    };
    *value = Tag::IntArray(uuid.to_vec());
    true
}

fn is_horse(id: &str) -> bool {
    matches!(
        id,
        "minecraft:donkey"
            | "minecraft:horse"
            | "minecraft:llama"
            | "minecraft:mule"
            | "minecraft:skeleton_horse"
            | "minecraft:trader_llama"
            | "minecraft:zombie_horse"
    )
}

fn is_tameable(id: &str) -> bool {
    matches!(id, "minecraft:cat" | "minecraft:parrot" | "minecraft:wolf")
}

fn is_animal(id: &str) -> bool {
    is_horse(id)
        || is_tameable(id)
        || matches!(
            id,
            "minecraft:bee"
                | "minecraft:chicken"
                | "minecraft:cow"
                | "minecraft:fox"
                | "minecraft:mooshroom"
                | "minecraft:ocelot"
                | "minecraft:panda"
                | "minecraft:pig"
                | "minecraft:polar_bear"
                | "minecraft:rabbit"
                | "minecraft:sheep"
                | "minecraft:turtle"
                | "minecraft:hoglin"
        )
}

fn is_mob(id: &str) -> bool {
    is_animal(id)
        || matches!(
            id,
            "minecraft:bat"
                | "minecraft:blaze"
                | "minecraft:cave_spider"
                | "minecraft:cod"
                | "minecraft:creeper"
                | "minecraft:dolphin"
                | "minecraft:drowned"
                | "minecraft:elder_guardian"
                | "minecraft:ender_dragon"
                | "minecraft:enderman"
                | "minecraft:endermite"
                | "minecraft:evoker"
                | "minecraft:ghast"
                | "minecraft:giant"
                | "minecraft:guardian"
                | "minecraft:husk"
                | "minecraft:illusioner"
                | "minecraft:magma_cube"
                | "minecraft:pufferfish"
                | "minecraft:zombified_piglin"
                | "minecraft:salmon"
                | "minecraft:shulker"
                | "minecraft:silverfish"
                | "minecraft:skeleton"
                | "minecraft:slime"
                | "minecraft:snow_golem"
                | "minecraft:spider"
                | "minecraft:squid"
                | "minecraft:stray"
                | "minecraft:tropical_fish"
                | "minecraft:vex"
                | "minecraft:villager"
                | "minecraft:iron_golem"
                | "minecraft:vindicator"
                | "minecraft:pillager"
                | "minecraft:wandering_trader"
                | "minecraft:witch"
                | "minecraft:wither"
                | "minecraft:wither_skeleton"
                | "minecraft:zombie"
                | "minecraft:zombie_villager"
                | "minecraft:phantom"
                | "minecraft:ravager"
                | "minecraft:piglin"
        )
}

fn is_projectile(id: &str) -> bool {
    matches!(
        id,
        "minecraft:arrow"
            | "minecraft:dragon_fireball"
            | "minecraft:firework_rocket"
            | "minecraft:fireball"
            | "minecraft:llama_spit"
            | "minecraft:small_fireball"
            | "minecraft:snowball"
            | "minecraft:spectral_arrow"
            | "minecraft:egg"
            | "minecraft:ender_pearl"
            | "minecraft:experience_bottle"
            | "minecraft:potion"
            | "minecraft:trident"
            | "minecraft:wither_skull"
    )
}

/// v2514 BlockEntityUUIDFix: conduit target_uuid{M,L} -> Target int-array
/// (ML-tag helper); skull Owner -> SkullOwner whenever Owner is present
/// (Id string converted when valid, Owner removed in all Owner-present cases).
fn block_entity_uuid(tag: &mut Tag) -> bool {
    let id = nbt::get(tag, "id").and_then(nbt::string).unwrap_or("");
    if id == "minecraft:conduit" {
        return replace_uuid_ml(tag, "target_uuid", "Target");
    }
    if id == "minecraft:skull" {
        let Some(mut owner) = nbt::take(tag, "Owner") else {
            return false;
        };
        replace_uuid_string(&mut owner, "Id", "Id");
        nbt::set(tag, "SkullOwner", owner);
        return true;
    }
    false
}

/// v2516 GossipUUIDFix: Gossips[].Target Most/Least -> Target int-array.
fn gossip_uuid(tag: &mut Tag) -> bool {
    let Some(gossips) = nbt::get_mut(tag, "Gossips").and_then(nbt::list_mut) else {
        return false;
    };
    let mut changed = false;
    for gossip in gossips.iter_mut() {
        changed |= old_player_map::replace_uuid(gossip, "Target", "Target");
    }
    changed
}

/// v2518 JigsawPropertiesFix: name=attachement_type ALWAYS (unconditional
/// overwrite), target=attachement_type, pool=target_pool; missing names
/// default to minecraft:empty.
fn jigsaw_properties(tag: &mut Tag) -> bool {
    let attachment = nbt::get(tag, "attachement_type")
        .and_then(nbt::string)
        .unwrap_or("minecraft:empty")
        .to_owned();
    let pool = nbt::get(tag, "target_pool")
        .and_then(nbt::string)
        .unwrap_or("minecraft:empty")
        .to_owned();
    nbt::set(tag, "name", text(&attachment));
    nbt::set(tag, "target", text(&attachment));
    nbt::remove(tag, "attachement_type");
    nbt::set(tag, "pool", text(&pool));
    nbt::remove(tag, "target_pool");
    true
}

/// v2533 VillagerFollowRangeFix: Attributes[] generic.follow_range Base
/// 16.0 -> 48.0.
fn villager_follow_range(tag: &mut Tag) -> bool {
    let Some(attributes) = nbt::get_mut(tag, "Attributes").and_then(nbt::list_mut) else {
        return false;
    };
    let mut changed = false;
    for attribute in attributes.iter_mut() {
        if nbt::get(attribute, "Name").and_then(nbt::string) != Some("generic.follow_range") {
            continue;
        }
        if nbt::get(attribute, "Base").and_then(nbt::number) == Some(16.0) {
            nbt::set(attribute, "Base", Tag::Double(48.0));
            changed = true;
        }
    }
    changed
}

/// v2535 EntityShulkerRotationFix: rebuild the complete list as doubles and
/// subtract 180 from its first element (non-numeric elements default to 180).
fn shulker_rotation(tag: &mut Tag) -> bool {
    let Some(Tag::List(rotation)) = nbt::get_mut(tag, "Rotation") else {
        return false;
    };
    if rotation.elements.is_empty() {
        return false;
    }
    for (index, value) in rotation.elements.iter_mut().enumerate() {
        let angle = nbt::number(value).unwrap_or(180.0);
        *value = Tag::Double(if index == 0 { angle - 180.0 } else { angle });
    }
    rotation.element_type = mc_nbt::tag_type::DOUBLE;
    true
}

fn the_renamening(id: &str) -> Option<&'static str> {
    Some(match id {
        "minecraft:commandblock_minecart" => "minecraft:command_block_minecart",
        "minecraft:ender_crystal" => "minecraft:end_crystal",
        "minecraft:snowman" => "minecraft:snow_golem",
        "minecraft:evocation_illager" => "minecraft:evoker",
        "minecraft:evocation_fangs" => "minecraft:evoker_fangs",
        "minecraft:illusion_illager" => "minecraft:illusioner",
        "minecraft:vindication_illager" => "minecraft:vindicator",
        "minecraft:villager_golem" => "minecraft:iron_golem",
        "minecraft:xp_orb" => "minecraft:experience_orb",
        "minecraft:xp_bottle" => "minecraft:experience_bottle",
        "minecraft:eye_of_ender_signal" => "minecraft:eye_of_ender",
        "minecraft:fireworks_rocket" => "minecraft:firework_rocket",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_at(tag: &mut Tag, kind: NodeKind, version: i32) -> bool {
        let mut report = UpgradeReport::default();
        fix_node(tag, kind, version, &mut report).expect("old_entities never fails");
        after_children(tag, kind, version, &mut report).expect("legacy post-child fix");
        !report.steps.is_empty()
    }

    fn entity(id: &str, fields: Vec<(&str, Tag)>) -> Tag {
        let mut all = vec![("id", text(id))];
        all.extend(fields);
        nbt::compound(all)
    }

    #[test]
    fn legacy_villager_loot_pickup_is_unconditionally_enabled() {
        for previous in [None, Some(Tag::Byte(0)), Some(Tag::Byte(1))] {
            let mut villager = entity("Villager", vec![]);
            if let Some(previous) = previous {
                nbt::set(&mut villager, "CanPickUpLoot", previous);
            }
            apply_at(&mut villager, NodeKind::Entity, 101);
            assert_eq!(nbt::get(&villager, "CanPickUpLoot"), Some(&Tag::Byte(1)));
        }
        let mut cow = entity("Cow", vec![("CanPickUpLoot", Tag::Byte(0))]);
        apply_at(&mut cow, NodeKind::Entity, 101);
        assert_eq!(nbt::get(&cow, "CanPickUpLoot"), Some(&Tag::Byte(0)));
    }

    #[test]
    fn potion_schema_revision_runs_after_owner_fix_but_skips_revision_zero_source() {
        for from in [2510, 2511] {
            let mut potion = entity(
                "minecraft:potion",
                vec![
                    (
                        "Potion",
                        nbt::compound(vec![("id", text("minecraft:splash_potion"))]),
                    ),
                    (
                        "owner",
                        nbt::compound(vec![("M", Tag::Long(21)), ("L", Tag::Long(22))]),
                    ),
                ],
            );
            let mut report = UpgradeReport {
                from,
                ..UpgradeReport::default()
            };
            fix_node(&mut potion, NodeKind::Entity, 2511, &mut report).unwrap();
            assert!(nbt::get(&potion, "Potion").is_none());
            assert!(nbt::get(&potion, "Item").is_some());
            assert_eq!(nbt::get(&potion, "OwnerUUID").is_some(), from == 2510);
            assert_eq!(nbt::get(&potion, "owner").is_some(), from == 2511);
        }
    }

    #[test]
    fn source_808_receives_the_later_shulker_revision() {
        let mut shulker = entity("minecraft:shulker", vec![]);
        let mut report = UpgradeReport {
            from: 808,
            ..UpgradeReport::default()
        };
        fix_node(&mut shulker, NodeKind::Entity, 808, &mut report).unwrap();
        assert_eq!(nbt::get(&shulker, "Color"), Some(&Tag::Byte(10)));
    }

    #[test]
    fn equipment_splits_and_drop_chances() {
        let mut tag = entity(
            "minecraft:zombie",
            vec![
                (
                    "Equipment",
                    Tag::List(mc_nbt::ListTag {
                        element_type: mc_nbt::tag_type::COMPOUND,
                        elements: vec![
                            entity("minecraft:stone_sword", vec![("Count", Tag::Byte(1))]),
                            entity("minecraft:iron_helmet", vec![("Count", Tag::Byte(1))]),
                        ],
                    }),
                ),
                (
                    "DropChances",
                    Tag::List(mc_nbt::ListTag {
                        element_type: mc_nbt::tag_type::FLOAT,
                        elements: vec![Tag::Float(0.5), Tag::Float(0.0)],
                    }),
                ),
            ],
        );
        assert!(apply_at(&mut tag, NodeKind::Entity, 100));
        assert!(nbt::get(&tag, "Equipment").is_none());
        assert!(nbt::get(&tag, "DropChances").is_none());
        let hand = nbt::list(nbt::get(&tag, "HandItems").unwrap()).unwrap();
        assert_eq!(hand.len(), 2);
        let chances = nbt::list(nbt::get(&tag, "HandDropChances").unwrap()).unwrap();
        assert_eq!(chances.len(), 2);
    }

    #[test]
    fn minecart_type_and_riding_and_string_uuid() {
        let mut cart = entity("Minecart", vec![("Type", Tag::Int(1))]);
        assert!(apply_at(&mut cart, NodeKind::Entity, 107));
        assert_eq!(nbt::get(&cart, "id"), Some(&text("MinecartChest")));
        assert!(nbt::get(&cart, "Type").is_none());

        let mut rider = entity(
            "minecraft:pig",
            vec![("Riding", entity("minecraft:cow", vec![]))],
        );
        assert!(apply_at(&mut rider, NodeKind::Entity, 135));
        assert!(nbt::get(&rider, "Riding").is_none());
        assert_eq!(
            nbt::list(nbt::get(&rider, "Passengers").unwrap())
                .unwrap()
                .len(),
            1
        );

        let mut with_uuid = entity(
            "minecraft:pig",
            vec![("UUID", text("12345678-1234-1234-1234-123456789abc"))],
        );
        assert!(apply_at(&mut with_uuid, NodeKind::Entity, 108));
        assert!(nbt::get(&with_uuid, "UUIDMost").is_some());
        assert!(nbt::get(&with_uuid, "UUIDLeast").is_some());
        assert!(nbt::get(&with_uuid, "UUID").is_none());
    }

    #[test]
    fn health_healf_and_horse_saddle_and_redundant_chances() {
        let mut zombie = entity("Zombie", vec![("HealF", Tag::Float(10.0))]);
        assert!(apply_at(&mut zombie, NodeKind::Entity, 109));
        assert_eq!(nbt::get(&zombie, "Health"), Some(&Tag::Float(10.0)));
        assert!(nbt::get(&zombie, "HealF").is_none());

        let mut horse = entity("EntityHorse", vec![("Saddle", Tag::Byte(1))]);
        assert!(apply_at(&mut horse, NodeKind::Entity, 110));
        assert!(nbt::get(&horse, "SaddleItem").is_some());
        assert!(nbt::get(&horse, "Saddle").is_none());

        let mut mob = entity(
            "minecraft:zombie",
            vec![
                (
                    "HandDropChances",
                    Tag::List(mc_nbt::ListTag {
                        element_type: mc_nbt::tag_type::FLOAT,
                        elements: vec![Tag::Float(0.0), Tag::Float(0.0)],
                    }),
                ),
                (
                    "ArmorDropChances",
                    Tag::List(mc_nbt::ListTag {
                        element_type: mc_nbt::tag_type::FLOAT,
                        elements: vec![
                            Tag::Float(0.0),
                            Tag::Float(0.0),
                            Tag::Float(0.0),
                            Tag::Float(0.0),
                        ],
                    }),
                ),
            ],
        );
        assert!(apply_at(&mut mob, NodeKind::Entity, 113));
        assert!(nbt::get(&mob, "HandDropChances").is_none());
        assert!(nbt::get(&mob, "ArmorDropChances").is_none());
    }

    #[test]
    fn splits_and_id_maps() {
        let mut guardian = entity("Guardian", vec![("Elder", Tag::Byte(1))]);
        assert!(apply_at(&mut guardian, NodeKind::Entity, 700));
        assert_eq!(nbt::get(&guardian, "id"), Some(&text("ElderGuardian")));

        let mut skeleton = entity("Skeleton", vec![("SkeletonType", Tag::Int(1))]);
        assert!(apply_at(&mut skeleton, NodeKind::Entity, 701));
        assert_eq!(nbt::get(&skeleton, "id"), Some(&text("WitherSkeleton")));

        let mut zombie = entity("Zombie", vec![("ZombieType", Tag::Int(6))]);
        assert!(apply_at(&mut zombie, NodeKind::Entity, 702));
        assert_eq!(nbt::get(&zombie, "id"), Some(&text("Husk")));

        let mut zv = entity("Zombie", vec![("ZombieType", Tag::Int(2))]);
        assert!(apply_at(&mut zv, NodeKind::Entity, 702));
        assert_eq!(nbt::get(&zv, "id"), Some(&text("ZombieVillager")));
        assert_eq!(nbt::get(&zv, "Profession"), Some(&Tag::Int(1)));

        let mut horse = entity("EntityHorse", vec![("Type", Tag::Int(1))]);
        assert!(apply_at(&mut horse, NodeKind::Entity, 703));
        assert_eq!(nbt::get(&horse, "id"), Some(&text("Donkey")));

        // 502 invalid-profession fallback: deterministic UUIDMost.rem_euclid(6).
        let mut bad_prof = entity(
            "Zombie",
            vec![
                ("IsVillager", Tag::Byte(1)),
                ("VillagerProfession", Tag::Int(9)),
                ("UUIDMost", Tag::Long(14)),
            ],
        );
        assert!(apply_at(&mut bad_prof, NodeKind::Entity, 502));
        assert_eq!(nbt::get(&bad_prof, "ZombieType"), Some(&Tag::Int(2)));
        assert!(nbt::get(&bad_prof, "IsVillager").is_none());
        // Same input twice -> same output (no RNG).
        let mut bad_prof2 = entity(
            "Zombie",
            vec![
                ("IsVillager", Tag::Byte(1)),
                ("VillagerProfession", Tag::Int(9)),
                ("UUIDMost", Tag::Long(14)),
            ],
        );
        assert!(apply_at(&mut bad_prof2, NodeKind::Entity, 502));
        assert_eq!(nbt::get(&bad_prof2, "ZombieType"), Some(&Tag::Int(2)));
        // No UUID -> 0; outcome always in 0..6.
        let mut no_uuid = entity(
            "Zombie",
            vec![
                ("IsVillager", Tag::Byte(1)),
                ("VillagerProfession", Tag::Int(-3)),
            ],
        );
        assert!(apply_at(&mut no_uuid, NodeKind::Entity, 502));
        assert_eq!(nbt::get(&no_uuid, "ZombieType"), Some(&Tag::Int(0)));

        let mut orb = entity("XPOrb", vec![]);
        assert!(apply_at(&mut orb, NodeKind::Entity, 705));
        assert_eq!(nbt::get(&orb, "id"), Some(&text("minecraft:xp_orb")));

        let mut be = entity("Chest", vec![]);
        assert!(apply_at(&mut be, NodeKind::BlockEntity, 704));
        assert_eq!(nbt::get(&be, "id"), Some(&text("minecraft:chest")));
    }

    #[test]
    fn spawner_identifiers_and_shulker_and_trades() {
        let mut spawner = entity("MobSpawner", vec![("EntityId", text("Creeper"))]);
        assert!(apply_at(&mut spawner, NodeKind::BlockEntity, 106));
        assert_eq!(nbt::get(&spawner, "id"), Some(&text("MobSpawner")));
        let data = nbt::get(&spawner, "SpawnData").unwrap();
        assert_eq!(nbt::get(data, "id"), Some(&text("Creeper")));

        // MinecartSpawner entity: nested UNTAGGED_SPAWNER fields transform,
        // outer entity id is untouched (pre-705 legacy id kept for EntityIdFix).
        let mut cart = entity("MinecartSpawner", vec![("EntityId", text("Creeper"))]);
        assert!(apply_at(&mut cart, NodeKind::Entity, 106));
        assert_eq!(nbt::get(&cart, "id"), Some(&text("MinecartSpawner")));
        assert!(nbt::get(&cart, "EntityId").is_none());

        // Untouched spawner-minecart (no EntityId/Potentials): no-op, id kept.
        let mut bare = entity("minecraft:spawner_minecart", vec![]);
        assert!(!apply_at(&mut bare, NodeKind::Entity, 106));
        assert_eq!(
            nbt::get(&bare, "id"),
            Some(&text("minecraft:spawner_minecart"))
        );

        let mut shulker = entity("minecraft:shulker", vec![]);
        assert!(apply_at(&mut shulker, NodeKind::Entity, 808));
        assert_eq!(nbt::get(&shulker, "Color"), Some(&Tag::Byte(10)));

        let mut box_be = entity("minecraft:shulker_box", vec![("Color", Tag::Byte(3))]);
        assert!(apply_at(&mut box_be, NodeKind::BlockEntity, 813));
        assert!(nbt::get(&box_be, "Color").is_none());

        let mut villager = entity(
            "minecraft:villager",
            vec![(
                "Offers",
                nbt::compound(vec![(
                    "Recipes",
                    Tag::List(mc_nbt::ListTag {
                        element_type: mc_nbt::tag_type::COMPOUND,
                        elements: vec![nbt::compound(vec![(
                            "sell",
                            nbt::compound(vec![("id", text("minecraft:carved_pumpkin"))]),
                        )])],
                    }),
                )]),
            )],
        );
        assert!(apply_at(&mut villager, NodeKind::Entity, 1451));
        let recipes =
            nbt::list(nbt::get(nbt::get(&villager, "Offers").unwrap(), "Recipes").unwrap())
                .unwrap();
        assert_eq!(
            nbt::get(nbt::get(&recipes[0], "sell").unwrap(), "id"),
            Some(&text("minecraft:pumpkin"))
        );
    }

    #[test]
    fn directions_motives_names_and_wolf() {
        let mut frame = entity("minecraft:item_frame", vec![("Facing", Tag::Byte(0))]);
        assert!(apply_at(&mut frame, NodeKind::Entity, 1456));
        assert_eq!(nbt::get(&frame, "Facing"), Some(&Tag::Byte(3)));

        let mut painting = entity("minecraft:painting", vec![("Motive", text("donkeykong"))]);
        assert!(apply_at(&mut painting, NodeKind::Entity, 1460));
        assert_eq!(
            nbt::get(&painting, "Motive"),
            Some(&text("minecraft:donkey_kong"))
        );

        let mut named = entity("minecraft:pig", vec![("CustomName", text("Bob"))]);
        assert!(apply_at(&mut named, NodeKind::Entity, 1458));
        assert_eq!(
            nbt::get(&named, "CustomName"),
            Some(&Tag::String("{\"text\":\"Bob\"}".into()))
        );

        let mut wolf = entity("minecraft:wolf", vec![("CollarColor", Tag::Int(3))]);
        assert!(apply_at(&mut wolf, NodeKind::Entity, 1451));
        assert_eq!(nbt::get(&wolf, "CollarColor"), Some(&Tag::Byte(12)));
    }

    #[test]
    fn cat_villager_profession_and_rebuilds() {
        let mut ocelot = entity(
            "minecraft:ocelot",
            vec![("CatType", Tag::Int(2)), ("Owner", text("Steve"))],
        );
        assert!(apply_at(&mut ocelot, NodeKind::Entity, 1904));
        assert_eq!(nbt::get(&ocelot, "id"), Some(&text("minecraft:cat")));

        let mut cat = entity("minecraft:cat", vec![("CatType", Tag::Int(9))]);
        assert!(apply_at(&mut cat, NodeKind::Entity, 1917));
        assert_eq!(nbt::get(&cat, "CatType"), Some(&Tag::Int(10)));

        let mut villager = entity(
            "minecraft:villager",
            vec![("Profession", Tag::Int(0)), ("Career", Tag::Int(2))],
        );
        assert!(apply_at(&mut villager, NodeKind::Entity, 1918));
        let data = nbt::get(&villager, "VillagerData").unwrap();
        assert_eq!(
            nbt::get(data, "profession"),
            Some(&text("minecraft:fisherman"))
        );

        let mut ravager = entity("minecraft:illager_beast", vec![]);
        assert!(apply_at(&mut ravager, NodeKind::Entity, 1928));
        assert_eq!(nbt::get(&ravager, "id"), Some(&text("minecraft:ravager")));

        let mut gossip_villager = entity(
            "minecraft:villager",
            vec![(
                "Gossips",
                Tag::List(mc_nbt::ListTag {
                    element_type: mc_nbt::tag_type::COMPOUND,
                    elements: vec![
                        nbt::compound(vec![("Type", text("golem"))]),
                        nbt::compound(vec![("Type", text("major_positive"))]),
                    ],
                }),
            )],
        );
        assert!(apply_at(&mut gossip_villager, NodeKind::Entity, 1963));
        assert_eq!(
            nbt::list(nbt::get(&gossip_villager, "Gossips").unwrap())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn uuid_families_and_projectile_and_gossip() {
        let mut horse = entity(
            "minecraft:horse",
            vec![("UUIDMost", Tag::Long(1)), ("UUIDLeast", Tag::Long(2))],
        );
        assert!(apply_at(&mut horse, NodeKind::Entity, 2514));
        assert!(matches!(nbt::get(&horse, "UUID"), Some(Tag::IntArray(_))));
        assert!(nbt::get(&horse, "UUIDMost").is_none());

        let mut arrow = entity(
            "minecraft:arrow",
            vec![
                ("OwnerUUIDMost", Tag::Long(3)),
                ("OwnerUUIDLeast", Tag::Long(4)),
            ],
        );
        assert!(apply_at(&mut arrow, NodeKind::Entity, 2511));
        assert!(matches!(
            nbt::get(&arrow, "OwnerUUID"),
            Some(Tag::IntArray(_))
        ));

        let mut gossip = entity(
            "minecraft:villager",
            vec![(
                "Gossips",
                Tag::List(mc_nbt::ListTag {
                    element_type: mc_nbt::tag_type::COMPOUND,
                    elements: vec![nbt::compound(vec![
                        ("TargetMost", Tag::Long(5)),
                        ("TargetLeast", Tag::Long(6)),
                    ])],
                }),
            )],
        );
        assert!(apply_at(&mut gossip, NodeKind::Entity, 2516));
        let entries = nbt::list(nbt::get(&gossip, "Gossips").unwrap()).unwrap();
        assert!(matches!(
            nbt::get(&entries[0], "Target"),
            Some(Tag::IntArray(_))
        ));

        let mut conduit = entity(
            "minecraft:conduit",
            vec![(
                "target_uuid",
                nbt::compound(vec![("M", Tag::Long(7)), ("L", Tag::Long(8))]),
            )],
        );
        assert!(apply_at(&mut conduit, NodeKind::BlockEntity, 2514));
        assert!(matches!(
            nbt::get(&conduit, "Target"),
            Some(Tag::IntArray(_))
        ));

        let mut jigsaw = entity(
            "minecraft:jigsaw",
            vec![
                ("attachement_type", text("minecraft:empty")),
                ("target_pool", text("minecraft:empty")),
            ],
        );
        assert!(apply_at(&mut jigsaw, NodeKind::BlockEntity, 2518));
        assert_eq!(nbt::get(&jigsaw, "pool"), Some(&text("minecraft:empty")));

        let mut shulker = entity(
            "minecraft:shulker",
            vec![(
                "Rotation",
                Tag::List(mc_nbt::ListTag {
                    element_type: mc_nbt::tag_type::DOUBLE,
                    elements: vec![Tag::Double(200.0), Tag::Double(0.0)],
                }),
            )],
        );
        assert!(apply_at(&mut shulker, NodeKind::Entity, 2535));
        assert_eq!(
            nbt::list(nbt::get(&shulker, "Rotation").unwrap()).unwrap()[0],
            Tag::Double(20.0)
        );
    }

    #[test]
    fn attributes_memory_follow_range_and_misc_renames() {
        let mut mob = entity(
            "minecraft:zombie",
            vec![(
                "Attributes",
                Tag::List(mc_nbt::ListTag {
                    element_type: mc_nbt::tag_type::COMPOUND,
                    elements: vec![nbt::compound(vec![("Name", text("generic.maxHealth"))])],
                }),
            )],
        );
        assert!(apply_at(&mut mob, NodeKind::Entity, 2523));
        let attrs = nbt::list(nbt::get(&mob, "Attributes").unwrap()).unwrap();
        assert_eq!(
            nbt::get(&attrs[0], "Name"),
            Some(&text("minecraft:generic.max_health"))
        );

        let mut villager = entity(
            "minecraft:villager",
            vec![(
                "Brain",
                nbt::compound(vec![(
                    "memories",
                    nbt::compound(vec![("minecraft:meeting_point", Tag::Int(3))]),
                )]),
            )],
        );
        assert!(apply_at(&mut villager, NodeKind::Entity, 2505));
        let memories = nbt::get(nbt::get(&villager, "Brain").unwrap(), "memories").unwrap();
        assert!(
            nbt::get(
                nbt::get(memories, "minecraft:meeting_point").unwrap(),
                "value"
            )
            .is_some()
        );

        let mut follower = entity(
            "minecraft:villager",
            vec![(
                "Attributes",
                Tag::List(mc_nbt::ListTag {
                    element_type: mc_nbt::tag_type::COMPOUND,
                    elements: vec![nbt::compound(vec![
                        ("Name", text("generic.follow_range")),
                        ("Base", Tag::Double(16.0)),
                    ])],
                }),
            )],
        );
        assert!(apply_at(&mut follower, NodeKind::Entity, 2533));
        let attrs = nbt::list(nbt::get(&follower, "Attributes").unwrap()).unwrap();
        assert_eq!(nbt::get(&attrs[0], "Base"), Some(&Tag::Double(48.0)));

        let mut strider = entity("minecraft:strider", vec![("NoGravity", Tag::Byte(1))]);
        assert!(apply_at(&mut strider, NodeKind::Entity, 2529));
        assert_eq!(nbt::get(&strider, "NoGravity"), Some(&Tag::Byte(0)));

        let mut pigman = entity("minecraft:zombie_pigman", vec![]);
        assert!(apply_at(&mut pigman, NodeKind::Entity, 2509));
        assert_eq!(
            nbt::get(&pigman, "id"),
            Some(&text("minecraft:zombified_piglin"))
        );

        let mut puffer = entity("minecraft:puffer_fish", vec![]);
        assert!(apply_at(&mut puffer, NodeKind::Entity, 1483));
        assert_eq!(nbt::get(&puffer, "id"), Some(&text("minecraft:pufferfish")));

        let mut shulker = entity("minecraft:shulker", vec![("Color", Tag::Byte(10))]);
        assert!(apply_at(&mut shulker, NodeKind::Entity, 1474));
        assert_eq!(nbt::get(&shulker, "Color"), Some(&Tag::Byte(16)));
    }

    #[test]
    fn block_state_upgrade_and_numeric_states() {
        // Numeric piston state resolves through the exact getTag table.
        let mut piston = entity(
            "minecraft:piston",
            vec![("blockId", Tag::Int(1)), ("blockData", Tag::Int(0))],
        );
        assert!(apply_at(&mut piston, NodeKind::BlockEntity, 1451));
        assert!(nbt::get(&piston, "blockId").is_none());
        assert!(nbt::get(&piston, "blockState").is_some());
        // Falling-block numeric TileID resolves; Data removed.
        let mut falling = entity(
            "minecraft:falling_block",
            vec![("TileID", Tag::Int(1)), ("Data", Tag::Byte(0))],
        );
        assert!(apply_at(&mut falling, NodeKind::Entity, 1451));
        assert!(nbt::get(&falling, "TileID").is_none());
        assert!(nbt::get(&falling, "Data").is_none());
        assert!(nbt::get(&falling, "BlockState").is_some());
    }

    #[test]
    fn remove_block_entity_tag_falling_block_branch() {
        // 1451r5 ENTITY branch only: falling_block TileEntityData gated on the
        // TAG's nested id.
        let mut noteblock = entity(
            "minecraft:falling_block",
            vec![(
                "TileEntityData",
                nbt::compound(vec![("id", text("minecraft:noteblock"))]),
            )],
        );
        assert!(apply_at(&mut noteblock, NodeKind::Entity, 1451));
        assert!(nbt::get(&noteblock, "TileEntityData").is_none());

        let mut pot = entity(
            "minecraft:falling_block",
            vec![(
                "TileEntityData",
                nbt::compound(vec![("id", text("minecraft:flower_pot"))]),
            )],
        );
        assert!(apply_at(&mut pot, NodeKind::Entity, 1451));
        assert!(nbt::get(&pot, "TileEntityData").is_none());

        // Non-drop id keeps its TileEntityData.
        let mut chest = entity(
            "minecraft:falling_block",
            vec![(
                "TileEntityData",
                nbt::compound(vec![("id", text("minecraft:chest"))]),
            )],
        );
        let _ = apply_at(&mut chest, NodeKind::Entity, 1451);
        assert!(nbt::get(&chest, "TileEntityData").is_some());

        // Wrong holder: non-falling entity never drops.
        let mut zombie = entity(
            "minecraft:zombie",
            vec![(
                "TileEntityData",
                nbt::compound(vec![("id", text("minecraft:noteblock"))]),
            )],
        );
        let _ = apply_at(&mut zombie, NodeKind::Entity, 1451);
        assert!(nbt::get(&zombie, "TileEntityData").is_some());
    }

    #[test]
    fn furnace_recipe_fold() {
        // Synthetic: RecipesUsedSize 2, one positive + one zero amount.
        let mut furnace = entity(
            "minecraft:furnace",
            vec![
                ("RecipesUsedSize", Tag::Int(2)),
                ("RecipeLocation0", text("minecraft:bread")),
                ("RecipeAmount0", Tag::Int(3)),
                ("RecipeLocation1", text("minecraft:iron_ingot")),
                ("RecipeAmount1", Tag::Int(0)),
            ],
        );
        assert!(apply_at(&mut furnace, NodeKind::BlockEntity, 2501));
        assert!(nbt::get(&furnace, "RecipesUsedSize").is_none());
        assert!(nbt::get(&furnace, "RecipeLocation0").is_none());
        assert!(nbt::get(&furnace, "RecipeAmount1").is_none());
        let used = nbt::get(&furnace, "RecipesUsed").unwrap();
        assert_eq!(nbt::get(used, "minecraft:bread"), Some(&Tag::Int(3)));
        assert!(nbt::get(used, "minecraft:iron_ingot").is_none());

        // Missing size -> still stamps empty RecipesUsed (vanilla unconditional).
        let mut bare = entity("minecraft:smoker", vec![]);
        assert!(apply_at(&mut bare, NodeKind::BlockEntity, 2501));
        assert!(nbt::fields(nbt::get(&bare, "RecipesUsed").unwrap()).is_some());

        // Wrong holder: entity never rewrites.
        let mut mob = entity("minecraft:furnace", vec![("RecipesUsedSize", Tag::Int(1))]);
        let _ = apply_at(&mut mob, NodeKind::Entity, 2501);
        assert!(nbt::get(&mob, "RecipesUsed").is_none());
    }

    #[test]
    fn splash_potion_item_field_rename() {
        // 2511r1: Potion -> Item on potion entities only.
        let mut potion = entity("minecraft:potion", vec![("Potion", nbt::compound(vec![]))]);
        assert!(apply_at(&mut potion, NodeKind::Entity, 2511));
        assert!(nbt::get(&potion, "Potion").is_none());
        assert!(nbt::get(&potion, "Item").is_some());

        // No Potion key -> r1 no-op (r0 may still apply via owner).
        let mut bare = entity("minecraft:potion", vec![]);
        let _ = apply_at(&mut bare, NodeKind::Entity, 2511);
        assert!(nbt::get(&bare, "Item").is_none());

        // Wrong holder: arrow keeps its Potion key.
        let mut arrow = entity("minecraft:arrow", vec![("Potion", nbt::compound(vec![]))]);
        let _ = apply_at(&mut arrow, NodeKind::Entity, 2511);
        assert!(nbt::get(&arrow, "Potion").is_some());
    }

    #[test]
    fn riding_three_deep_preserves_each_mount_edge() {
        let mut chain = entity(
            "Pig",
            vec![(
                "Riding",
                entity("Cow", vec![("Riding", entity("Boat", vec![]))]),
            )],
        );
        assert!(apply_at(&mut chain, NodeKind::Entity, 135));
        assert_eq!(nbt::get(&chain, "id"), Some(&text("Boat")));
        let cow = &nbt::list(nbt::get(&chain, "Passengers").unwrap()).unwrap()[0];
        assert_eq!(nbt::get(cow, "id"), Some(&text("Cow")));
        let pig = &nbt::list(nbt::get(cow, "Passengers").unwrap()).unwrap()[0];
        assert_eq!(nbt::get(pig, "id"), Some(&text("Pig")));
        assert!(nbt::get(pig, "Riding").is_none());
    }

    #[test]
    fn numeric_entity_states_keep_metadata_and_use_vanilla_defaults() {
        for fields in [
            vec![("TileID", Tag::Int(17)), ("Data", Tag::Byte(3))],
            vec![("Tile", Tag::Byte(17)), ("Data", Tag::Byte(3))],
            vec![
                ("Block", text("minecraft:log")),
                ("TileID", Tag::Int(1)),
                ("Data", Tag::Byte(3)),
            ],
        ] {
            let mut falling = entity("minecraft:falling_block", fields);
            apply_at(&mut falling, NodeKind::Entity, 1451);
            assert_eq!(
                nbt::get(&falling, "BlockState"),
                Some(&super::super::old_block_states::state(17 << 4 | 3))
            );
            for key in ["Block", "TileID", "Tile", "Data"] {
                assert!(nbt::get(&falling, key).is_none());
            }
        }
        let mut piston = entity("minecraft:piston", vec![]);
        apply_at(&mut piston, NodeKind::BlockEntity, 1451);
        assert_eq!(
            nbt::get(&piston, "blockState"),
            Some(&super::super::old_block_states::state(0))
        );
        for name in [
            "egg",
            "ender_pearl",
            "fireball",
            "potion",
            "small_fireball",
            "snowball",
            "wither_skull",
            "xp_bottle",
        ] {
            let mut projectile = entity(
                &format!("minecraft:{name}"),
                vec![("inTile", Tag::Int(1)), ("inData", Tag::Byte(3))],
            );
            apply_at(&mut projectile, NodeKind::Entity, 1451);
            assert!(nbt::get(&projectile, "inTile").is_none());
            assert_eq!(nbt::get(&projectile, "inData"), Some(&Tag::Byte(3)));
        }
    }

    #[test]
    fn uuid_families_inherit_living_rewrites_without_inventing_animal_owners() {
        let attributes = || {
            Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: vec![nbt::compound(vec![(
                    "Modifiers",
                    Tag::List(mc_nbt::ListTag {
                        element_type: mc_nbt::tag_type::COMPOUND,
                        elements: vec![nbt::compound(vec![
                            ("UUIDMost", Tag::Long(0x1234_5678_1234_5679)),
                            ("UUIDLeast", Tag::Long(0x2345_6789_2345_678b)),
                        ])],
                    }),
                )])],
            })
        };
        for id in [
            "minecraft:horse",
            "minecraft:wolf",
            "minecraft:cow",
            "minecraft:zombie",
            "minecraft:armor_stand",
            "mod:opaque",
        ] {
            let mut tag = entity(
                id,
                vec![
                    ("Attributes", attributes()),
                    ("OwnerUUID", text("1-2-3-4-5")),
                    ("LoveCauseMost", Tag::Long(1)),
                    ("LoveCauseLeast", Tag::Long(2)),
                    (
                        "Leash",
                        nbt::compound(vec![
                            ("UUIDMost", Tag::Long(3)),
                            ("UUIDLeast", Tag::Long(4)),
                        ]),
                    ),
                ],
            );
            apply_at(&mut tag, NodeKind::Entity, 2514);
            let attribute = &nbt::list(nbt::get(&tag, "Attributes").unwrap()).unwrap()[0];
            let modifier = &nbt::list(nbt::get(attribute, "Modifiers").unwrap()).unwrap()[0];
            assert_eq!(
                nbt::get(modifier, "UUID").is_some(),
                id != "mod:opaque",
                "{id}"
            );
            assert_eq!(
                nbt::get(&tag, "Owner").is_some(),
                is_horse(id) || is_tameable(id),
                "{id}"
            );
            assert_eq!(nbt::get(&tag, "LoveCause").is_some(), is_animal(id), "{id}");
            let leash = nbt::get(&tag, "Leash").unwrap();
            assert_eq!(nbt::get(leash, "UUID").is_some(), is_mob(id), "{id}");
        }
    }

    #[test]
    fn uuid_compounds_keep_all_long_bits_and_projectile_moves_overwrite_owner() {
        let most = 0x1234_5678_1234_5679_i64;
        let least = 0x2345_6789_2345_678b_i64;
        let expected = Tag::IntArray(vec![0x1234_5678, 0x1234_5679, 0x2345_6789, 0x2345_678b]);
        let mut item = entity(
            "minecraft:item",
            vec![(
                "Owner",
                nbt::compound(vec![("M", Tag::Long(most)), ("L", Tag::Long(least))]),
            )],
        );
        apply_at(&mut item, NodeKind::Entity, 2514);
        assert_eq!(nbt::get(&item, "Owner"), Some(&expected));
        for value in [expected, text("opaque-owner")] {
            let mut arrow = entity(
                "minecraft:arrow",
                vec![("OwnerUUID", value.clone()), ("Owner", text("old-owner"))],
            );
            apply_at(&mut arrow, NodeKind::Entity, 2514);
            assert!(nbt::get(&arrow, "OwnerUUID").is_none());
            assert_eq!(nbt::get(&arrow, "Owner"), Some(&value));
        }
    }

    #[test]
    fn established_villagers_receive_xp_without_rebuilding_level() {
        for (level, xp) in [(2, 10), (3, 50), (4, 100), (5, 150)] {
            for previous in [None, Some(text("invalid"))] {
                let mut tag = entity(
                    "minecraft:villager",
                    vec![(
                        "VillagerData",
                        nbt::compound(vec![("level", Tag::Int(level))]),
                    )],
                );
                if let Some(previous) = previous {
                    nbt::set(&mut tag, "Xp", previous);
                }
                apply_at(&mut tag, NodeKind::Entity, 1955);
                assert_eq!(nbt::get(&tag, "Xp"), Some(&Tag::Int(xp)));
                assert_eq!(
                    nbt::get(nbt::get(&tag, "VillagerData").unwrap(), "level"),
                    Some(&Tag::Int(level))
                );
            }
        }
    }

    #[test]
    fn shulker_rotation_retypes_every_element() {
        let mut tag = entity(
            "minecraft:shulker",
            vec![(
                "Rotation",
                Tag::List(mc_nbt::ListTag {
                    element_type: mc_nbt::tag_type::FLOAT,
                    elements: vec![Tag::Float(200.0), Tag::Float(45.0)],
                }),
            )],
        );
        apply_at(&mut tag, NodeKind::Entity, 2535);
        assert_eq!(
            nbt::get(&tag, "Rotation"),
            Some(&Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::DOUBLE,
                elements: vec![Tag::Double(20.0), Tag::Double(45.0)],
            }))
        );
    }

    #[test]
    fn jukebox_uses_full_item_tables_and_preserves_original_record() {
        for (record, expected) in [
            (1, Some("minecraft:stone")),
            (2256, Some("minecraft:music_disc_13")),
            (276, None),
        ] {
            let original_item = nbt::compound(vec![("id", text("minecraft:dirt"))]);
            let mut tag = entity(
                "minecraft:jukebox",
                vec![
                    ("Record", Tag::Int(record)),
                    ("RecordItem", original_item.clone()),
                ],
            );
            apply_at(&mut tag, NodeKind::BlockEntity, 1451);
            assert_eq!(nbt::get(&tag, "Record"), Some(&Tag::Int(record)));
            if let Some(expected) = expected {
                let stack = nbt::get(&tag, "RecordItem").unwrap();
                assert_eq!(nbt::get(stack, "id"), Some(&text(expected)));
                assert_eq!(nbt::get(stack, "Count"), Some(&Tag::Byte(1)));
            } else {
                assert_eq!(nbt::get(&tag, "RecordItem"), Some(&original_item));
            }
        }
    }

    #[test]
    fn item_frame_direction_and_dir_take_distinct_vanilla_branches() {
        let mut directional = entity(
            "ItemFrame",
            vec![
                ("Direction", Tag::Byte(3)),
                ("Dir", Tag::Byte(1)),
                ("ItemRotation", Tag::Byte(3)),
                ("TileX", Tag::Int(10)),
            ],
        );
        apply_at(&mut directional, NodeKind::Entity, 111);
        assert_eq!(nbt::get(&directional, "Facing"), Some(&Tag::Byte(3)));
        assert_eq!(nbt::get(&directional, "TileX"), Some(&Tag::Int(11)));
        assert_eq!(nbt::get(&directional, "ItemRotation"), Some(&Tag::Byte(6)));
        assert_eq!(nbt::get(&directional, "Dir"), Some(&Tag::Byte(1)));
        assert!(nbt::get(&directional, "Direction").is_none());

        let mut legacy = entity(
            "ItemFrame",
            vec![
                ("Dir", Tag::Byte(1)),
                ("ItemRotation", Tag::Byte(3)),
                ("TileX", Tag::Int(10)),
            ],
        );
        apply_at(&mut legacy, NodeKind::Entity, 111);
        assert_eq!(nbt::get(&legacy, "Facing"), Some(&Tag::Byte(1)));
        assert_eq!(nbt::get(&legacy, "TileX"), Some(&Tag::Int(10)));
        assert_eq!(nbt::get(&legacy, "ItemRotation"), Some(&Tag::Byte(3)));
        assert!(nbt::get(&legacy, "Dir").is_none());
    }
}
