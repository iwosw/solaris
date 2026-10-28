//! 26.3 boundary shapes, from local DataFixers registrations 4996/5006.
//! The native chunk/entity readers keep their 4790 block-state representation;
//! offline export converts typed nodes back to the actual 5023 representation.
use super::{NATIVE_26_3_DATA_VERSION as DATA_VERSION, NodeKind, UpgradeError, UpgradeReport, nbt};
use mc_nbt::Tag;

pub(super) fn fix_node(
    tag: &mut Tag,
    kind: NodeKind,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if kind == NodeKind::BlockState {
        if let Tag::String(name) = tag {
            // BlockState.CODEC in 26.3 uses a bare block identifier for its
            // default state (Either<BuiltInRegistries.BLOCK, FULL_CODEC>).
            if report.to != DATA_VERSION {
                let name = std::mem::take(name);
                *tag = Tag::Compound(vec![("Name".into(), Tag::String(name))]);
            }
            report.applied(DATA_VERSION, "26.3 block-state identifier shorthand");
            return Ok(());
        }
        if nbt::fields(tag).is_none() {
            return Err(UpgradeError::malformed(
                "BlockState",
                "expected identifier or compound",
            ));
        }
    }
    // A same-format traversal inspects typed children without rewriting them.
    // Reuse the walker so opaque/custom-data payloads cannot promote a record.
    if report.from == DATA_VERSION && report.to == DATA_VERSION {
        let components_need_new_format = kind == NodeKind::Components
            && nbt::fields(tag).is_some_and(|fields| {
                fields.iter().any(|(name, value)| {
                    use mc_data::data_component_types_26_1_2 as ids;
                    let name = name.trim_start_matches('!');
                    let new_component = ids::ITEM_COMPONENT_CODECS.iter().any(|(candidate, id)| {
                        *candidate == name
                            && matches!(
                                *id,
                                ids::ATTACK_ANIMATION
                                    | ids::INTERACT_ANIMATION
                                    | ids::BLOCK_TRANSFORMER
                                    | ids::VILLAGER_FOOD
                                    | ids::SULFUR_CUBE_CONTENT
                                    | ids::COMPOSTABLE
                                    | ids::COOKING_FUEL
                                    | ids::BREWING_FUEL
                                    | ids::MOB_VISIBILITY
                                    | ids::PROVIDES_POTTERY_PATTERN
                                    | ids::SIGN_TEXT_FRONT
                                    | ids::SIGN_TEXT_BACK
                                    | ids::WAXED
                                    | ids::CUSHION_COLOR
                            )
                    });
                    new_component
                        || (name == "minecraft:pot_decorations"
                            && matches!(value, Tag::Compound(_)))
                })
            });
        let pot_needs_new_format = kind == NodeKind::BlockEntity
            && nbt::get(tag, "id").and_then(nbt::string) == Some("minecraft:decorated_pot")
            && matches!(nbt::get(tag, "sherds"), Some(Tag::Compound(_)));
        if components_need_new_format || pot_needs_new_format {
            report.applied(DATA_VERSION, "26.3 item/template shape");
        }
        return Ok(());
    }
    if kind == NodeKind::BlockState {
        let (old_id, new_id, old_properties, new_properties) = if report.to == DATA_VERSION {
            ("Name", "id", "Properties", "properties")
        } else {
            ("id", "Name", "properties", "Properties")
        };
        for (old, new) in [(old_id, new_id), (old_properties, new_properties)] {
            if nbt::get(tag, old).is_some() && nbt::get(tag, new).is_some() {
                return Err(UpgradeError::malformed(
                    "block_state",
                    "duplicate old/new field spelling",
                ));
            }
            nbt::rename(tag, old, new);
        }
    }
    let pot = if kind == NodeKind::Components {
        nbt::get_mut(tag, "minecraft:pot_decorations")
    } else if kind == NodeKind::BlockEntity
        && nbt::get(tag, "id").and_then(nbt::string) == Some("minecraft:decorated_pot")
    {
        nbt::get_mut(tag, "sherds")
    } else {
        None
    };
    if let Some(pot) = pot {
        mc_data::item_nbt::upgrade_pot_decorations(pot)
            .map_err(|error| UpgradeError::malformed("pot_decorations", error.to_string()))?;
    }
    Ok(())
}

/// Export a native 4790-shaped record as genuinely 26.3-shaped NBT. This is not
/// a version-stamp-only bump: every typed block-state and pot node is migrated.
pub fn prepare_export(tag: &mut Tag, kind: super::RecordKind) -> Result<(), UpgradeError> {
    let mut report = UpgradeReport {
        from: super::TARGET_DATA_VERSION,
        to: DATA_VERSION,
        ..Default::default()
    };
    let (node, wrapped) = match kind {
        super::RecordKind::Chunk => (NodeKind::Chunk, false),
        super::RecordKind::EntityRegion => (NodeKind::EntityRegion, false),
        super::RecordKind::Entity => (NodeKind::Entity, false),
        super::RecordKind::Player => (NodeKind::Player, false),
        super::RecordKind::Level => (NodeKind::Level, nbt::get(tag, "Data").is_some()),
        super::RecordKind::Item => (NodeKind::Item, false),
    };
    let outer_stamp = wrapped && nbt::get(tag, "DataVersion").is_some();
    let root = if wrapped {
        nbt::get_mut(tag, "Data").unwrap()
    } else {
        &mut *tag
    };
    if kind == super::RecordKind::Chunk
        && let Some(blending) = nbt::get_mut(root, "blending_data")
        && let Some(Tag::List(heights)) = nbt::get_mut(blending, "heights")
        && heights.element_type != mc_nbt::tag_type::FLOAT
    {
        // DataFixer 5009: the absent-height sentinel changes with the
        // storage type; casting Double.MAX_VALUE directly produces infinity.
        for height in &mut heights.elements {
            let value = nbt::number(height).unwrap_or(f64::MAX);
            *height = Tag::Float(if value == f64::MAX {
                f32::MAX
            } else {
                value as f32
            });
        }
        heights.element_type = mc_nbt::tag_type::FLOAT;
    }
    super::walk::visit(root, node, DATA_VERSION, &mut report)?;
    nbt::set(root, "DataVersion", Tag::Int(DATA_VERSION));
    if kind == super::RecordKind::Level
        && let Some(version) = nbt::get_mut(root, "Version")
    {
        nbt::set(version, "Name", Tag::String("26.3".into()));
        nbt::set(version, "Id", Tag::Int(DATA_VERSION));
    }
    if outer_stamp {
        nbt::set(tag, "DataVersion", Tag::Int(DATA_VERSION));
    }
    Ok(())
}

/// Inspect a native record without mutation. Ordinary 4790 records keep their
/// export version; original 26.3 chunks and actual new component/template shapes
/// require the matching 26.3 writer.
pub fn requires_26_3(tag: &mut Tag, kind: super::RecordKind) -> Result<bool, UpgradeError> {
    if nbt::get(tag, "DataVersion").and_then(nbt::int) == Some(DATA_VERSION) {
        return Ok(true);
    }
    let node = match kind {
        super::RecordKind::Chunk => NodeKind::Chunk,
        super::RecordKind::EntityRegion => NodeKind::EntityRegion,
        super::RecordKind::Entity => NodeKind::Entity,
        super::RecordKind::Player => NodeKind::Player,
        super::RecordKind::Level => NodeKind::Level,
        super::RecordKind::Item => NodeKind::Item,
    };
    let root = if kind == super::RecordKind::Level && nbt::get(tag, "Data").is_some() {
        nbt::get_mut(tag, "Data").unwrap()
    } else {
        tag
    };
    let mut report = UpgradeReport {
        from: DATA_VERSION,
        to: DATA_VERSION,
        ..Default::default()
    };
    super::walk::visit(root, node, DATA_VERSION, &mut report)?;
    Ok(!report.steps.is_empty())
}
