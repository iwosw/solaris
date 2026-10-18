//! Lossless conversion between generic WIT world operations and native DTOs.
use crate::bindings::solaris::plugin::world_structures as wire;
use mc_script::{
    ScriptChunkAvailability, ScriptDtoError, ScriptStructureMaterial, ScriptStructureState,
    ScriptWorksiteFailure, ScriptWorksiteInteractionPoint, ScriptWorksiteRequest,
    ScriptWorldBounds, ScriptWorldFeature, ScriptWorldStructureOperation,
    ScriptWorldStructureResult,
};

pub(crate) fn decode(
    value: wire::WorldStructureOperation,
) -> Result<ScriptWorldStructureOperation, ScriptDtoError> {
    Ok(match value {
        wire::WorldStructureOperation::QueryBlocks(v) => {
            ScriptWorldStructureOperation::QueryBlocks {
                dimension: v.dimension,
                positions: v.positions.into_iter().map(position).collect(),
            }
        }
        wire::WorldStructureOperation::ListFeatures(v) => {
            ScriptWorldStructureOperation::ListFeatures {
                cursor: v.cursor,
                limit: v.limit,
            }
        }
        wire::WorldStructureOperation::QueryFeature(v) => {
            ScriptWorldStructureOperation::QueryFeature {
                feature_id: v.feature_id,
            }
        }
        wire::WorldStructureOperation::Survey(v) => ScriptWorldStructureOperation::Survey {
            dimension: v.dimension,
            bounds: bounds(v.bounds)?,
        },
        wire::WorldStructureOperation::PrepareStructure(v) => {
            ScriptWorldStructureOperation::PrepareStructure {
                operation_id: v.operation_id,
                dimension: v.dimension,
                blueprint_id: v.blueprint_id,
                anchor: position(v.anchor),
                rotation: v.rotation,
                survey_token: v.survey_token,
                expected_world_revision: v.expected_world_revision,
                expected_blueprint_hash: v.expected_blueprint_hash,
            }
        }
        wire::WorldStructureOperation::AdvanceStructure(v) => {
            ScriptWorldStructureOperation::AdvanceStructure {
                operation_id: v.operation_id,
                structure_id: v.structure_id,
                stage: v.stage,
                reservation_ref: v.reservation_ref,
                expected_revision: v.expected_revision,
                work_units: v.work_units,
            }
        }
        wire::WorldStructureOperation::PauseStructure(v) => {
            ScriptWorldStructureOperation::PauseStructure {
                operation_id: v.operation_id,
                structure_id: v.structure_id,
                expected_revision: v.expected_revision,
            }
        }
        wire::WorldStructureOperation::ResumeStructure(v) => {
            ScriptWorldStructureOperation::ResumeStructure {
                operation_id: v.operation_id,
                structure_id: v.structure_id,
                expected_revision: v.expected_revision,
            }
        }
        wire::WorldStructureOperation::CancelStructure(v) => {
            ScriptWorldStructureOperation::CancelStructure {
                operation_id: v.operation_id,
                structure_id: v.structure_id,
                expected_revision: v.expected_revision,
            }
        }
        wire::WorldStructureOperation::Status(v) => ScriptWorldStructureOperation::Status {
            structure_id: v.structure_id,
        },
        wire::WorldStructureOperation::CheckStructure(v) => {
            ScriptWorldStructureOperation::CheckStructure {
                structure_id: v.structure_id,
            }
        }
        wire::WorldStructureOperation::RegisterWorksite(v) => {
            ScriptWorldStructureOperation::RegisterWorksite {
                operation_id: v.operation_id,
                request: Box::new(ScriptWorksiteRequest::new(
                    bounds(v.request.footprint)?,
                    v.request
                        .interaction_points
                        .into_iter()
                        .map(decode_point)
                        .collect(),
                    v.request.survey_token,
                    v.request.expected_world_revision,
                )),
            }
        }
        wire::WorldStructureOperation::WorksiteStatus(v) => {
            ScriptWorldStructureOperation::WorksiteStatus {
                worksite_id: v.worksite_id,
            }
        }
        wire::WorldStructureOperation::CheckWorksite(v) => {
            ScriptWorldStructureOperation::CheckWorksite {
                worksite_id: v.worksite_id,
            }
        }
    })
}
fn position(v: wire::BlockPosition) -> [i32; 3] {
    [v.x, v.y, v.z]
}
fn block_position(v: [i32; 3]) -> wire::BlockPosition {
    wire::BlockPosition {
        x: v[0],
        y: v[1],
        z: v[2],
    }
}
fn bounds(v: wire::WorldBounds) -> Result<ScriptWorldBounds, ScriptDtoError> {
    ScriptWorldBounds::new(position(v.min), position(v.max))
}
fn wire_bounds(v: &ScriptWorldBounds) -> wire::WorldBounds {
    wire::WorldBounds {
        min: block_position(v.min),
        max: block_position(v.max),
    }
}
fn decode_point(v: wire::WorksiteInteractionPoint) -> ScriptWorksiteInteractionPoint {
    match v {
        wire::WorksiteInteractionPoint::Entrance(at) => {
            ScriptWorksiteInteractionPoint::Entrance { at: position(at) }
        }
        wire::WorksiteInteractionPoint::Block(v) => ScriptWorksiteInteractionPoint::Block {
            at: position(v.at),
            block: v.block,
        },
        wire::WorksiteInteractionPoint::Workstation(v) => {
            ScriptWorksiteInteractionPoint::Workstation {
                at: position(v.at),
                capability: v.capability,
            }
        }
        wire::WorksiteInteractionPoint::Container(at) => {
            ScriptWorksiteInteractionPoint::Container { at: position(at) }
        }
    }
}
fn encode_point(v: &ScriptWorksiteInteractionPoint) -> Option<wire::WorksiteInteractionPoint> {
    Some(match v {
        ScriptWorksiteInteractionPoint::Entrance { at } => {
            wire::WorksiteInteractionPoint::Entrance(block_position(*at))
        }
        ScriptWorksiteInteractionPoint::Block { at, block } => {
            wire::WorksiteInteractionPoint::Block(wire::WorksiteBlock {
                at: block_position(*at),
                block: block.clone(),
            })
        }
        ScriptWorksiteInteractionPoint::Workstation { at, capability } => {
            wire::WorksiteInteractionPoint::Workstation(wire::WorksiteWorkstation {
                at: block_position(*at),
                capability: capability.clone(),
            })
        }
        ScriptWorksiteInteractionPoint::Container { at } => {
            wire::WorksiteInteractionPoint::Container(block_position(*at))
        }
        _ => return None,
    })
}
fn feature(v: &ScriptWorldFeature) -> Option<wire::WorldFeature> {
    Some(wire::WorldFeature {
        feature_id: v.feature_id.clone(),
        kind: v.kind.clone(),
        dimension: v.dimension.clone(),
        bounds: wire_bounds(&v.bounds),
        contents_known: v.contents_known,
        blocks: v
            .blocks
            .iter()
            .map(|v| wire::WorldFeatureBlock {
                at: block_position(v.at),
                block: v.block.clone(),
            })
            .collect(),
        interaction_points: v
            .interaction_points
            .iter()
            .map(encode_point)
            .collect::<Option<Vec<_>>>()?,
        entity_ids: v.entity_ids.clone(),
    })
}
fn material(v: &ScriptStructureMaterial) -> wire::StructureMaterial {
    wire::StructureMaterial {
        resource: v.resource.clone(),
        quantity: v.quantity,
    }
}
fn state(v: ScriptStructureState) -> Option<wire::StructureState> {
    Some(match v {
        ScriptStructureState::Prepared => wire::StructureState::Prepared,
        ScriptStructureState::Running => wire::StructureState::Running,
        ScriptStructureState::Paused => wire::StructureState::Paused,
        ScriptStructureState::Committed => wire::StructureState::Committed,
        ScriptStructureState::Cancelled => wire::StructureState::Cancelled,
        _ => return None,
    })
}
fn availability(v: ScriptChunkAvailability) -> Option<wire::ChunkAvailability> {
    Some(match v {
        ScriptChunkAvailability::Loaded => wire::ChunkAvailability::Loaded,
        ScriptChunkAvailability::Unloaded => wire::ChunkAvailability::Unloaded,
        _ => return None,
    })
}
fn failure(v: ScriptWorksiteFailure) -> Option<wire::WorksiteFailure> {
    Some(match v {
        ScriptWorksiteFailure::Unloaded => wire::WorksiteFailure::Unloaded,
        ScriptWorksiteFailure::MissingBlock => wire::WorksiteFailure::MissingBlock,
        ScriptWorksiteFailure::MissingWorkstation => wire::WorksiteFailure::MissingWorkstation,
        ScriptWorksiteFailure::MissingContainer => wire::WorksiteFailure::MissingContainer,
        ScriptWorksiteFailure::BlockedEntrance => wire::WorksiteFailure::BlockedEntrance,
        ScriptWorksiteFailure::Unwalkable => wire::WorksiteFailure::Unwalkable,
        _ => return None,
    })
}

pub(crate) fn encode_result(v: &ScriptWorldStructureResult) -> Option<wire::WorldStructureResult> {
    Some(match v {
        ScriptWorldStructureResult::Blocks { query } => {
            wire::WorldStructureResult::Blocks(wire::WorldBlockQuery {
                world_revision: query.world_revision,
                blocks: query
                    .blocks
                    .iter()
                    .map(|v| wire::WorldBlockFacts {
                        at: block_position(v.at),
                        block: v.block.clone(),
                        properties: v
                            .properties
                            .iter()
                            .map(|v| wire::WorldBlockProperty {
                                key: v.key.clone(),
                                value: v.value.clone(),
                            })
                            .collect(),
                        standable: v.standable,
                        container: v.container,
                        door: v.door,
                        bed: v.bed,
                    })
                    .collect(),
            })
        }
        ScriptWorldStructureResult::Features { page } => {
            wire::WorldStructureResult::Features(wire::WorldFeaturePage {
                features: page
                    .features
                    .iter()
                    .map(feature)
                    .collect::<Option<Vec<_>>>()?,
                cursor: page.cursor.clone(),
            })
        }
        ScriptWorldStructureResult::Feature { feature: v } => {
            wire::WorldStructureResult::Feature(feature(v)?)
        }
        ScriptWorldStructureResult::Survey { survey: v } => {
            wire::WorldStructureResult::Survey(wire::SurveySnapshot {
                dimension: v.dimension.clone(),
                bounds: wire_bounds(&v.bounds),
                revision: v.revision,
                world_revision: v.world_revision,
                chunk_availability: availability(v.chunk_availability)?,
                survey_token: v.survey_token.clone(),
                usable_plots: v.usable_plots,
                water_columns: v.water_columns,
                existing_structures: v.existing_structures,
                biome_tags: v.biome_tags.clone(),
                resource_tags: v.resource_tags.clone(),
            })
        }
        ScriptWorldStructureResult::Structure { structure: v } => {
            wire::WorldStructureResult::Structure(wire::StructureSnapshot {
                structure_id: v.structure_id.clone(),
                blueprint_id: v.blueprint_id.clone(),
                dimension: v.dimension.clone(),
                state: state(v.state)?,
                revision: v.revision,
                origin: block_position(v.origin),
                rotation: v.rotation,
                reserved_footprint: block_position(v.reserved_footprint),
                stages: v
                    .stages
                    .iter()
                    .map(|v| wire::StructureStagePlan {
                        stage: v.stage.clone(),
                        block_count: v.block_count,
                        work_units: v.work_units,
                        materials: v.materials.iter().map(material).collect(),
                    })
                    .collect(),
                current_stage_index: v.current_stage_index,
                completed_work_units: v.completed_work_units,
                resource_plan_hash: v.resource_plan_hash.clone(),
                reservation_ref: v.reservation_ref.clone(),
                watermark: v.watermark,
                consumed: v.consumed.iter().map(material).collect(),
                remaining: v.remaining.iter().map(material).collect(),
                pause_reason: v.pause_reason.clone(),
                interaction_points: v
                    .interaction_points
                    .iter()
                    .map(encode_point)
                    .collect::<Option<Vec<_>>>()?,
            })
        }
        ScriptWorldStructureResult::Receipt { receipt: v } => {
            wire::WorldStructureResult::Receipt(wire::StructureReceipt {
                structure_id: v.structure_id.clone(),
                stage: v.stage.clone(),
                sequence: v.sequence,
                block_count: v.block_count,
                work_units: v.work_units,
                consumed: v.consumed.iter().map(material).collect(),
                revision: v.revision,
            })
        }
        ScriptWorldStructureResult::Worksite { worksite: v } => {
            wire::WorldStructureResult::Worksite(wire::WorksiteSnapshot {
                worksite_id: v.worksite_id.clone(),
                footprint: wire_bounds(&v.footprint),
                interaction_points: v
                    .interaction_points
                    .iter()
                    .map(encode_point)
                    .collect::<Option<Vec<_>>>()?,
                world_revision: v.world_revision,
                revision: v.revision,
            })
        }
        ScriptWorldStructureResult::WorksiteCheck { check: v } => {
            wire::WorldStructureResult::WorksiteCheck(wire::WorksiteCheck {
                worksite_id: v.worksite_id.clone(),
                world_revision: v.world_revision,
                valid: v.valid,
                blocked_points: v
                    .blocked_points
                    .iter()
                    .map(|v| {
                        Some(wire::WorksitePointFailure {
                            at: block_position(v.at),
                            reason: failure(v.reason)?,
                        })
                    })
                    .collect::<Option<Vec<_>>>()?,
            })
        }
        ScriptWorldStructureResult::StructureCheck { check: v } => {
            wire::WorldStructureResult::StructureCheck(wire::StructureCheck {
                structure_id: v.structure_id.clone(),
                plugin_id: v.plugin_id.clone(),
                world_revision: v.world_revision,
                valid: v.valid,
                blocked_points: v
                    .blocked_points
                    .iter()
                    .map(|v| {
                        Some(wire::WorksitePointFailure {
                            at: block_position(v.at),
                            reason: failure(v.reason)?,
                        })
                    })
                    .collect::<Option<Vec<_>>>()?,
            })
        }
        _ => return None,
    })
}

/// The largest guest string, for the same staging charge as other native families.
pub(crate) fn max_text_bytes(v: &wire::WorldStructureOperation) -> usize {
    match v {
        wire::WorldStructureOperation::QueryBlocks(v) => v.dimension.len(),
        wire::WorldStructureOperation::ListFeatures(v) => v.cursor.as_ref().map_or(0, String::len),
        wire::WorldStructureOperation::QueryFeature(v) => v.feature_id.len(),
        wire::WorldStructureOperation::Survey(v) => v.dimension.len(),
        wire::WorldStructureOperation::PrepareStructure(v) => [
            v.operation_id.len(),
            v.dimension.len(),
            v.blueprint_id.len(),
            v.survey_token.len(),
            v.expected_blueprint_hash.as_ref().map_or(0, String::len),
        ]
        .into_iter()
        .max()
        .unwrap_or(0),
        wire::WorldStructureOperation::AdvanceStructure(v) => [
            v.operation_id.len(),
            v.structure_id.len(),
            v.stage.len(),
            v.reservation_ref.len(),
        ]
        .into_iter()
        .max()
        .unwrap_or(0),
        wire::WorldStructureOperation::PauseStructure(v)
        | wire::WorldStructureOperation::ResumeStructure(v)
        | wire::WorldStructureOperation::CancelStructure(v) => {
            v.operation_id.len().max(v.structure_id.len())
        }
        wire::WorldStructureOperation::Status(v)
        | wire::WorldStructureOperation::CheckStructure(v) => v.structure_id.len(),
        wire::WorldStructureOperation::RegisterWorksite(v) => v
            .request
            .interaction_points
            .iter()
            .map(|p| match p {
                wire::WorksiteInteractionPoint::Block(v) => v.block.len(),
                wire::WorksiteInteractionPoint::Workstation(v) => v.capability.len(),
                _ => 0,
            })
            .max()
            .unwrap_or(0)
            .max(v.operation_id.len())
            .max(v.request.survey_token.len()),
        wire::WorldStructureOperation::WorksiteStatus(v)
        | wire::WorldStructureOperation::CheckWorksite(v) => v.worksite_id.len(),
    }
}
