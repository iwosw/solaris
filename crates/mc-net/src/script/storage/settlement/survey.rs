//! Survey authorizations and the bounded survey operation.
//!
//! Split out of `settlement.rs` by CP-REFACTOR-040 with the same rule as the
//! warehouse module: the ledger authority and the shared atomic `apply` stay in
//! the parent, which declares this child and dispatches into it.

use super::*;

/// Storage transactions one survey token stays valid for.
const SURVEY_TOKEN_TTL_TRANSACTIONS: u64 = 64;

/// One durable, bounded survey authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::script::storage) struct DurableSurveyToken {
    pub(super) token: String,
    pub(super) plugin_id: String,
    pub(super) dimension: String,
    pub(super) bounds: ScriptSurveyBounds,
    pub(super) world_revision: u64,
    pub(super) expires_transaction: u64,
    pub(super) revision: u64,
}

impl SettlementLedger {
    pub(super) fn survey(&self, token: &str) -> Option<&DurableSurveyToken> {
        self.surveys.get(token)
    }
}

pub(super) fn validate_survey_token(
    survey: &DurableSurveyToken,
) -> Result<(), PluginStorageStartError> {
    validate_identifier(&survey.token, MAX_SURVEY_TOKEN_BYTES)?;
    validate_identifier(&survey.plugin_id, 128)?;
    validate_identifier(&survey.dimension, 128)?;
    survey
        .bounds
        .validate()
        .map_err(|_| PluginStorageStartError::Malformed("settlement survey bounds"))?;
    if survey.world_revision > MAX_SCRIPT_WORLD_TIME
        || survey.expires_transaction > MAX_SCRIPT_WORLD_TIME
    {
        return Err(PluginStorageStartError::Malformed(
            "settlement survey revision",
        ));
    }
    validate_revision(survey.revision)
}

impl crate::script::storage::InventoryRuntime {
    pub(super) fn survey(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        dimension: &str,
        bounds: ScriptSurveyBounds,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(world) = self.settlement_world() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        if bounds.validate().is_err() {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        let reading = match world.survey(plugin_id, dimension, bounds) {
            Ok(reading) => reading,
            Err(failure) => return Ok(rejected(failure)),
        };
        if reading.chunk_availability != ScriptChunkAvailability::Loaded {
            return Ok(rejected(ScriptOperationFailure::Unloaded));
        }
        let transaction_id = storage
            .revision
            .checked_add(1)
            .ok_or(PluginStorageMutationError::RevisionOverflow)?;
        let token = survey_token(plugin_id, transaction_id, dimension, bounds);
        let snapshot = ScriptSurveySnapshot::new(
            dimension.to_owned(),
            bounds,
            transaction_id,
            reading.revision,
            reading.chunk_availability,
            token.clone(),
            reading.usable_plots,
            reading.water_columns,
            reading.claimed,
            reading.existing_structures,
            reading.biome_tags,
            reading.resource_tags,
        );
        let (batch, outcome) = match storage.prepare_settlement_change_batch(
            plugin_id,
            settlement_payload(ScriptSettlementResult::Survey {
                survey: Box::new(snapshot),
            }),
            vec![DurableSettlementChange::Survey {
                survey: Box::new(DurableSurveyToken {
                    token,
                    plugin_id: plugin_id.to_owned(),
                    dimension: dimension.to_owned(),
                    bounds,
                    world_revision: reading.revision,
                    expires_transaction: transaction_id
                        .saturating_add(SURVEY_TOKEN_TTL_TRANSACTIONS),
                    revision: 0,
                }),
            }],
        )? {
            ScriptStoragePrepareOutcome::Prepared(prepared) => prepared,
            ScriptStoragePrepareOutcome::Rejected => {
                return Ok(rejected(ScriptOperationFailure::Capacity));
            }
        };
        storage.commit_batch(batch)?;
        Ok(outcome)
    }
}
