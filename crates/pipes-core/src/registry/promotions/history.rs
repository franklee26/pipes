use rusqlite::{Connection, Row, params_from_iter};

use super::{
    Registry, assessment_hash, assessment_status, evaluate_criterion, matches_deployment,
    matches_harness, skipped_optional,
};
use crate::{
    ConsumerDeployment, HarnessCounts, HarnessResult, PromotionApproval, PromotionAssessment,
    PromotionCriterion, PromotionStatus, Result, validation,
};

const SELECT_RESULT: &str = "SELECT id, app_id, app_name, stage_id, check_id, release_ref, total, \
    passed, notes, manifest_hash, manifest_path, source_path, registration_updated_at, recorded_at \
    FROM harness_results";
const SELECT_APPROVAL: &str =
    "SELECT id, app_id, assessment, skipped_optional, notes, recorded_at FROM promotion_approvals";

impl Registry {
    pub(in crate::registry) fn load_harness_results(
        &self,
        connection: &Connection,
        app_id: Option<&str>,
    ) -> Result<Vec<HarnessResult>> {
        let filter = if app_id.is_some() {
            " WHERE app_id = ?1"
        } else {
            ""
        };
        let mut statement = connection
            .prepare(&format!("{SELECT_RESULT}{filter} ORDER BY id DESC"))
            .map_err(|error| self.storage("list harness results", error))?;
        let rows = statement
            .query_map(params_from_iter(app_id), StoredResult::read)
            .map_err(|error| self.storage("list harness results", error))?;
        rows.map(|row| {
            row.map_err(|error| self.storage("read harness result", error))?
                .decode(self)
        })
        .collect()
    }

    pub(in crate::registry) fn load_promotion_approvals(
        &self,
        connection: &Connection,
        app_id: Option<&str>,
    ) -> Result<Vec<PromotionApproval>> {
        let filter = if app_id.is_some() {
            " WHERE app_id = ?1"
        } else {
            ""
        };
        let mut statement = connection
            .prepare(&format!("{SELECT_APPROVAL}{filter} ORDER BY id DESC"))
            .map_err(|error| self.storage("list promotion approvals", error))?;
        let rows = statement
            .query_map(params_from_iter(app_id), StoredApproval::read)
            .map_err(|error| self.storage("list promotion approvals", error))?;
        let approvals = rows
            .map(|row| {
                row.map_err(|error| self.storage("read promotion approval", error))?
                    .decode(self)
            })
            .collect::<Result<Vec<_>>>()?;
        if !approvals.is_empty() {
            let results = self.load_harness_results(connection, app_id)?;
            let deployments = self.load_consumer_deployments(connection, app_id)?;
            for approval in &approvals {
                validate_evidence(&approval.assessment, &results, &deployments).map_err(
                    |message| {
                        self.storage(
                            "validate frozen approval evidence",
                            format!("approval '{}': {message}", approval.id),
                        )
                    },
                )?;
            }
        }
        Ok(approvals)
    }
}

struct StoredResult {
    id: i64,
    result: HarnessResult,
}

impl StoredResult {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        let id: i64 = row.get(0)?;
        Ok(Self {
            id,
            result: HarnessResult {
                id: id.to_string(),
                app_id: row.get(1)?,
                app_name: row.get(2)?,
                stage_id: row.get(3)?,
                check_id: row.get(4)?,
                release_ref: row.get(5)?,
                total: row.get(6)?,
                passed: row.get(7)?,
                notes: row.get(8)?,
                manifest_hash: row.get(9)?,
                manifest_path: row.get(10)?,
                source_path: row.get(11)?,
                registration_updated_at: row.get(12)?,
                recorded_at: row.get(13)?,
            },
        })
    }

    fn decode(self, registry: &Registry) -> Result<HarnessResult> {
        let result = &self.result;
        let invalid = |message: String| {
            registry.storage(
                "decode harness result",
                format!("result '{}': {message}", self.id),
            )
        };
        validation::insertion_id(&result.id).map_err(|message| invalid(message.to_owned()))?;
        for (field, value) in [
            ("appId", &result.app_id),
            ("stageId", &result.stage_id),
            ("checkId", &result.check_id),
        ] {
            validation::slug(value).map_err(|message| invalid(format!("{field}: {message}")))?;
        }
        validate_name(&result.app_name).map_err(invalid)?;
        validation::harness_counts(result.total, result.passed)
            .map_err(|message| invalid(message.to_owned()))?;
        validate_ref(&result.release_ref).map_err(invalid)?;
        validate_notes(result.notes.as_deref()).map_err(invalid)?;
        validation::manifest_hash(&result.manifest_hash)
            .map_err(|message| invalid(format!("manifestHash: {message}")))?;
        for (field, value) in [
            ("manifestPath", &result.manifest_path),
            ("sourcePath", &result.source_path),
        ] {
            validation::absolute_path(value)
                .map_err(|message| invalid(format!("{field}: {message}")))?;
        }
        for (field, value) in [
            ("registrationUpdatedAt", &result.registration_updated_at),
            ("recordedAt", &result.recorded_at),
        ] {
            validation::utc_timestamp(value)
                .map_err(|message| invalid(format!("{field}: {message}")))?;
        }
        Ok(self.result)
    }
}

struct StoredApproval {
    id: i64,
    app_id: String,
    assessment: String,
    skipped_optional: String,
    notes: Option<String>,
    recorded_at: String,
}

impl StoredApproval {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            app_id: row.get(1)?,
            assessment: row.get(2)?,
            skipped_optional: row.get(3)?,
            notes: row.get(4)?,
            recorded_at: row.get(5)?,
        })
    }

    fn decode(self, registry: &Registry) -> Result<PromotionApproval> {
        let invalid = |message: String| {
            registry.storage(
                "decode promotion approval",
                format!("approval '{}': {message}", self.id),
            )
        };
        let id = self.id.to_string();
        validation::insertion_id(&id).map_err(|message| invalid(message.to_owned()))?;
        let assessment: PromotionAssessment = serde_json::from_str(&self.assessment)
            .map_err(|error| invalid(format!("invalid assessment JSON: {error}")))?;
        let skips: Vec<String> = serde_json::from_str(&self.skipped_optional)
            .map_err(|error| invalid(format!("invalid optional skips JSON: {error}")))?;
        if self.app_id != assessment.app_id {
            return Err(invalid(
                "indexed app ID does not match the frozen assessment".to_owned(),
            ));
        }
        validate_assessment(&assessment).map_err(invalid)?;
        if skips != skipped_optional(&assessment) {
            return Err(invalid(
                "skippedOptional must exactly match warning criterion IDs in declaration order"
                    .to_owned(),
            ));
        }
        validate_notes(self.notes.as_deref()).map_err(invalid)?;
        validation::utc_timestamp(&self.recorded_at).map_err(invalid)?;
        Ok(PromotionApproval {
            id,
            assessment,
            skipped_optional: skips,
            notes: self.notes,
            recorded_at: self.recorded_at,
        })
    }
}

fn validate_name(value: &str) -> std::result::Result<(), String> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err("appName must be nonempty and contain no NUL characters".to_owned());
    }
    Ok(())
}

fn validate_ref(value: &str) -> std::result::Result<(), String> {
    if validation::normalize_ref(value)? != value {
        return Err("releaseRef must be stored in normalized form".to_owned());
    }
    Ok(())
}

fn validate_notes(value: Option<&str>) -> std::result::Result<(), String> {
    if validation::normalize_notes(value)?.as_deref() != value {
        return Err("notes must be stored in normalized form".to_owned());
    }
    Ok(())
}

fn validate_assessment(assessment: &PromotionAssessment) -> std::result::Result<(), String> {
    for value in [&assessment.app_id, &assessment.stage_id] {
        validation::slug(value)?;
    }
    validate_name(&assessment.app_name)?;
    validate_ref(&assessment.release_ref)?;
    validation::manifest_hash(&assessment.manifest_hash)?;
    validation::manifest_hash(&assessment.assessment_hash)?;
    validation::absolute_path(&assessment.manifest_path)?;
    validation::absolute_path(&assessment.source_path)?;
    validation::utc_timestamp(&assessment.registration_updated_at)?;
    validation::promotion_criteria(
        assessment
            .criteria
            .iter()
            .map(|evaluation| &evaluation.criterion),
    )
    .map_err(|(field, message)| format!("criteria{field}: {message}"))?;
    for requirement in &assessment.stage_requirements {
        match requirement.as_str() {
            "checks.passed" | "previous.passed" | "approval.local" => {}
            other => {
                let id = other
                    .strip_prefix("prepare.")
                    .and_then(|value| value.strip_suffix(".passed"))
                    .ok_or_else(|| "unknown frozen stage requirement".to_owned())?;
                validation::slug(id)?;
            }
        }
    }
    for evaluation in &assessment.criteria {
        let expected = evaluate_criterion(
            &evaluation.criterion,
            evaluation.evidence_id.clone(),
            evaluation.observed,
            evaluation.consumer.clone(),
        )?;
        if expected != *evaluation {
            return Err(format!(
                "criterion '{}' has inconsistent status, explanation, or evidence",
                evaluation.criterion.id()
            ));
        }
    }
    if assessment.status != assessment_status(&assessment.criteria)
        || !matches!(
            assessment.status,
            PromotionStatus::Ready | PromotionStatus::ReadyWithWarnings
        )
    {
        return Err(
            "an approval requires a configured, ready assessment with no blocked criteria"
                .to_owned(),
        );
    }
    if assessment_hash(assessment)
        .map_err(|error| format!("cannot fingerprint assessment: {error}"))?
        != assessment.assessment_hash
    {
        return Err("frozen assessment hash does not match its canonical contents".to_owned());
    }
    Ok(())
}

fn validate_evidence(
    assessment: &PromotionAssessment,
    results: &[HarnessResult],
    deployments: &[ConsumerDeployment],
) -> std::result::Result<(), String> {
    for evaluation in &assessment.criteria {
        let Some(id) = &evaluation.evidence_id else {
            continue;
        };
        let valid = match &evaluation.criterion {
            PromotionCriterion::Harness { check_id, .. } => results.iter().any(|result| {
                result.id == *id
                    && matches_harness(result, assessment, check_id)
                    && evaluation.observed
                        == Some(HarnessCounts {
                            total: result.total,
                            passed: result.passed,
                        })
            }),
            PromotionCriterion::Deployment { .. } => {
                evaluation.consumer.as_ref().is_some_and(|consumer| {
                    deployments.iter().any(|deployment| {
                        deployment.id == *id && matches_deployment(deployment, assessment, consumer)
                    })
                })
            }
        };
        if !valid {
            return Err(format!(
                "criterion '{}' references missing, mismatched, or corrupt evidence '{id}'",
                evaluation.criterion.id()
            ));
        }
    }
    Ok(())
}
