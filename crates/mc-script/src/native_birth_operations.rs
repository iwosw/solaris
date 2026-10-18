//! Generic observations of successful native births; no ownership or social policy.
use crate::{
    MAX_SCRIPT_WORLD_TIME, ScriptDtoError, ScriptNpcLifecycle, ScriptNpcLoadedState,
    validate_entity_uuid,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNativeBirthOrigin {
    pub parents: [String; 2],
    pub started_tick: u64,
    pub birth_tick: u64,
}

impl ScriptNativeBirthOrigin {
    #[must_use]
    pub fn new(parents: [String; 2], started_tick: u64, birth_tick: u64) -> Self {
        Self {
            parents,
            started_tick,
            birth_tick,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        for parent in &self.parents {
            validate_entity_uuid(parent)?;
        }
        if self.parents[0] == self.parents[1]
            || self.started_tick > self.birth_tick
            || self.birth_tick > MAX_SCRIPT_WORLD_TIME
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// No opaque NPC handle is minted by observation. Adoption remains authenticated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNativeBirthObservation {
    pub entity_uuid: String,
    pub origin: ScriptNativeBirthOrigin,
    pub lifecycle: ScriptNpcLifecycle,
    pub dimension: String,
    /// Current native age exists only while the native body is loaded.
    pub loaded: Option<ScriptNpcLoadedState>,
}

impl ScriptNativeBirthObservation {
    #[must_use]
    pub fn new(
        entity_uuid: String,
        origin: ScriptNativeBirthOrigin,
        lifecycle: ScriptNpcLifecycle,
        dimension: String,
        loaded: Option<ScriptNpcLoadedState>,
    ) -> Self {
        Self {
            entity_uuid,
            origin,
            lifecycle,
            dimension,
            loaded,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_entity_uuid(&self.entity_uuid)?;
        self.origin.validate()?;
        crate::check_contract_resource_id(&self.dimension)?;
        if (self.lifecycle == ScriptNpcLifecycle::AliveLoaded) != self.loaded.is_some()
            || self.lifecycle == ScriptNpcLifecycle::Released
            || self.origin.parents.contains(&self.entity_uuid)
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if let Some(loaded) = &self.loaded {
            loaded.validate()?;
            if loaded.dimension != self.dimension
                || loaded.birth_origin.as_ref() != Some(&self.origin)
            {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        MAX_NPC_QUERY_HANDLES, ScriptDtoError, ScriptNpcOperation, ScriptOperation,
        ScriptOperationRequest,
    };

    #[test]
    fn native_birth_query_bounds_apply_before_duplicate_parent_collapse() {
        for (parents, limit) in [
            (vec!["a1".to_owned(); MAX_NPC_QUERY_HANDLES + 1], 64),
            (Vec::new(), 64),
            (vec!["a1".to_owned()], 0),
            (vec!["a1".to_owned()], 65),
        ] {
            let request = ScriptOperationRequest::try_new(
                "birth-query",
                ScriptOperation::Npc {
                    operation: ScriptNpcOperation::QueryBirths {
                        parent_handles: parents,
                        cursor: None,
                        limit,
                    },
                },
            );
            assert!(matches!(request, Err(ScriptDtoError::InvalidBounds)));
        }
    }
}
