use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use super::Registry;
use crate::{
    ApprovePromotion, ConsumerDeployment, ConsumerSummary, CriterionEvaluation, CriterionStatus,
    DeploymentRequirement, Error, HarnessCounts, HarnessResult, PromotionApproval,
    PromotionAssessment, PromotionContext, PromotionCriterion, PromotionStatus,
    RecordHarnessResult, RegisteredApp, Result, StageSummary, validation,
};

mod history;

impl Registry {
    /// Append caller-reported external counts for a declared check command.
    ///
    /// Reports are not executed, parsed, or independently verified. Even zero
    /// counts or totals different from the policy are retained as evidence.
    pub fn record_harness_result(&self, request: &RecordHarnessResult) -> Result<HarnessResult> {
        let release_ref = validate_context(
            &request.app_id,
            &request.stage_id,
            &request.expected_manifest_hash,
            &request.expected_updated_at,
            &request.release_ref,
            invalid_result,
        )?;
        validation::slug(&request.check_id)
            .map_err(|message| invalid_result("checkId", message.to_owned()))?;
        validation::harness_counts(request.total, request.passed)
            .map_err(|message| invalid_result("passed", message.to_owned()))?;
        let notes = validation::normalize_notes(request.notes.as_deref())
            .map_err(|message| invalid_result("notes", message))?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| self.storage("begin harness result reporting", error))?;
        self.require_version(&transaction)?;
        self.validate_schema(&transaction)?;
        let app = self.reviewed_app(
            &transaction,
            &request.app_id,
            &request.expected_manifest_hash,
            &request.expected_updated_at,
        )?;
        let stage = stage(&app, &request.stage_id)?;
        if !stage
            .checks
            .iter()
            .any(|check| check.id == request.check_id)
        {
            return Err(Error::CheckNotFound {
                app_id: request.app_id.clone(),
                stage_id: request.stage_id.clone(),
                check_id: request.check_id.clone(),
            });
        }
        let recorded_at = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
        let inserted = transaction
            .execute(
                "INSERT INTO harness_results \
                 (app_id, app_name, stage_id, check_id, release_ref, total, passed, notes, \
                  manifest_hash, manifest_path, source_path, registration_updated_at, recorded_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    app.manifest.app_id,
                    app.manifest.name,
                    request.stage_id,
                    request.check_id,
                    release_ref,
                    request.total,
                    request.passed,
                    notes,
                    app.manifest.manifest_hash,
                    app.manifest.manifest_path,
                    app.manifest.source_path,
                    app.updated_at,
                    recorded_at,
                ],
            )
            .map_err(|error| self.storage("append harness result", error))?;
        let id = transaction.last_insert_rowid();
        if inserted != 1 || id <= 0 {
            return Err(self.storage(
                "append harness result",
                "expected one result with a positive insertion ID",
            ));
        }
        let result = HarnessResult {
            id: id.to_string(),
            app_id: app.manifest.app_id,
            app_name: app.manifest.name,
            stage_id: request.stage_id.clone(),
            check_id: request.check_id.clone(),
            release_ref,
            total: request.total,
            passed: request.passed,
            notes,
            manifest_hash: app.manifest.manifest_hash,
            manifest_path: app.manifest.manifest_path,
            source_path: app.manifest.source_path,
            registration_updated_at: app.updated_at,
            recorded_at,
        };
        transaction
            .commit()
            .map_err(|error| self.storage("commit harness result", error))?;
        Ok(result)
    }

    /// List immutable external reports, newest insertion first, including removed apps.
    pub fn harness_results(&self, app_id: Option<&str>) -> Result<Vec<HarnessResult>> {
        if let Some(app_id) = app_id {
            validation::slug(app_id)
                .map_err(|message| invalid_result("appId", message.to_owned()))?;
        }
        self.load_harness_results(&self.connection()?, app_id)
    }

    /// Evaluate only declared promotion criteria against one consistent snapshot.
    ///
    /// `stage_requirements` are returned unchanged for display, not evaluated.
    /// Evidence must match this candidate and the exact saved registration.
    pub fn assess_promotion(&self, request: &PromotionContext) -> Result<PromotionAssessment> {
        let release_ref = validate_promotion_context(request)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| self.storage("begin promotion assessment", error))?;
        self.require_version(&transaction)?;
        self.validate_schema(&transaction)?;
        let assessment = self.assess_in(&transaction, request, release_ref)?;
        transaction
            .commit()
            .map_err(|error| self.storage("finish promotion assessment", error))?;
        Ok(assessment)
    }

    /// Reassess under an immediate transaction and freeze an explicitly approved
    /// policy decision. This never satisfies legacy gates or advances a stage.
    pub fn approve_promotion(&self, request: &ApprovePromotion) -> Result<PromotionApproval> {
        if !request.approved {
            return Err(Error::ApprovalRequired);
        }
        let context = PromotionContext {
            app_id: request.app_id.clone(),
            stage_id: request.stage_id.clone(),
            expected_manifest_hash: request.expected_manifest_hash.clone(),
            expected_updated_at: request.expected_updated_at.clone(),
            release_ref: request.release_ref.clone(),
        };
        let release_ref = validate_promotion_context(&context)?;
        validation::manifest_hash(&request.expected_assessment_hash)
            .map_err(|message| invalid_promotion("expectedAssessmentHash", message.to_owned()))?;
        let notes = validation::normalize_notes(request.notes.as_deref())
            .map_err(|message| invalid_promotion("notes", message))?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| self.storage("begin promotion approval", error))?;
        self.require_version(&transaction)?;
        self.validate_schema(&transaction)?;
        let assessment = self.assess_in(&transaction, &context, release_ref)?;
        if assessment.assessment_hash != request.expected_assessment_hash {
            return Err(Error::AssessmentChanged);
        }
        match assessment.status {
            PromotionStatus::NotConfigured => {
                return Err(Error::PromotionNotConfigured {
                    app_id: request.app_id.clone(),
                    stage_id: request.stage_id.clone(),
                });
            }
            PromotionStatus::Blocked => {
                return Err(Error::PromotionBlocked {
                    app_id: request.app_id.clone(),
                    stage_id: request.stage_id.clone(),
                });
            }
            PromotionStatus::ReadyWithWarnings if !request.acknowledge_optional => {
                return Err(Error::OptionalStepsUnacknowledged);
            }
            PromotionStatus::Ready | PromotionStatus::ReadyWithWarnings => {}
        }
        let skipped_optional = skipped_optional(&assessment);
        let assessment_json = serde_json::to_string(&assessment)
            .map_err(|error| self.storage("serialize promotion assessment", error))?;
        let skipped_json = serde_json::to_string(&skipped_optional)
            .map_err(|error| self.storage("serialize optional skips", error))?;
        let recorded_at = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
        let inserted = transaction.execute(
            "INSERT INTO promotion_approvals (app_id, assessment, skipped_optional, notes, recorded_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![assessment.app_id, assessment_json, skipped_json, notes, recorded_at],
        ).map_err(|error| self.storage("append promotion approval", error))?;
        let id = transaction.last_insert_rowid();
        if inserted != 1 || id <= 0 {
            return Err(self.storage(
                "append promotion approval",
                "expected one approval with a positive insertion ID",
            ));
        }
        let approval = PromotionApproval {
            id: id.to_string(),
            assessment,
            skipped_optional,
            notes,
            recorded_at,
        };
        transaction
            .commit()
            .map_err(|error| self.storage("commit promotion approval", error))?;
        Ok(approval)
    }

    /// List frozen approvals newest first without consulting current registrations.
    pub fn promotion_approvals(&self, app_id: Option<&str>) -> Result<Vec<PromotionApproval>> {
        if let Some(app_id) = app_id {
            validation::slug(app_id)
                .map_err(|message| invalid_promotion("appId", message.to_owned()))?;
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| self.storage("begin promotion history read", error))?;
        self.require_version(&transaction)?;
        self.validate_schema(&transaction)?;
        let approvals = self.load_promotion_approvals(&transaction, app_id)?;
        transaction
            .commit()
            .map_err(|error| self.storage("finish promotion history read", error))?;
        Ok(approvals)
    }

    pub(super) fn assess_in(
        &self,
        connection: &Connection,
        context: &PromotionContext,
        release_ref: String,
    ) -> Result<PromotionAssessment> {
        let app = self.reviewed_app(
            connection,
            &context.app_id,
            &context.expected_manifest_hash,
            &context.expected_updated_at,
        )?;
        let stage = stage(&app, &context.stage_id)?;
        let mut assessment = PromotionAssessment {
            app_id: app.manifest.app_id.clone(),
            app_name: app.manifest.name.clone(),
            stage_id: stage.id.clone(),
            release_ref,
            manifest_hash: app.manifest.manifest_hash.clone(),
            manifest_path: app.manifest.manifest_path.clone(),
            source_path: app.manifest.source_path.clone(),
            registration_updated_at: app.updated_at.clone(),
            status: PromotionStatus::NotConfigured,
            criteria: Vec::new(),
            stage_requirements: stage.requires.clone(),
            assessment_hash: String::new(),
        };
        // Validate rows before provenance filtering so corrupt keys cannot hide a
        // newer result and silently resurrect an older passing report.
        let results = self.load_harness_results(connection, None)?;
        let deployments = self.load_consumer_deployments(connection, None)?;
        for criterion in &stage.promotion_criteria {
            let (evidence_id, observed, consumer) = match criterion {
                PromotionCriterion::Harness { check_id, .. } => {
                    // Loaders return descending insertion order. Never seek an older pass.
                    let result = results
                        .iter()
                        .find(|result| matches_harness(result, &assessment, check_id));
                    (
                        result.map(|result| result.id.clone()),
                        result.map(|result| HarnessCounts {
                            total: result.total,
                            passed: result.passed,
                        }),
                        None,
                    )
                }
                PromotionCriterion::Deployment { consumer_id, .. } => {
                    let consumer = stage
                        .consumers
                        .iter()
                        .find(|consumer| consumer.id == *consumer_id)
                        .ok_or_else(|| {
                            self.storage(
                                "evaluate promotion",
                                "criterion references an undeclared consumer",
                            )
                        })?;
                    let deployment = deployments
                        .iter()
                        .find(|deployment| matches_deployment(deployment, &assessment, consumer));
                    (
                        deployment.map(|deployment| deployment.id.clone()),
                        None,
                        Some(consumer.clone()),
                    )
                }
            };
            let evaluation = evaluate_criterion(criterion, evidence_id, observed, consumer)
                .map_err(|message| self.storage("evaluate promotion criterion", message))?;
            assessment.criteria.push(evaluation);
        }
        assessment.status = assessment_status(&assessment.criteria);
        assessment.assessment_hash = assessment_hash(&assessment)
            .map_err(|error| self.storage("fingerprint promotion assessment", error))?;
        Ok(assessment)
    }
}

fn invalid_result(field: &str, message: String) -> Error {
    Error::InvalidHarnessResult {
        field: field.to_owned(),
        message,
    }
}

fn invalid_promotion(field: &str, message: String) -> Error {
    Error::InvalidPromotion {
        field: field.to_owned(),
        message,
    }
}

fn validate_promotion_context(request: &PromotionContext) -> Result<String> {
    validate_context(
        &request.app_id,
        &request.stage_id,
        &request.expected_manifest_hash,
        &request.expected_updated_at,
        &request.release_ref,
        invalid_promotion,
    )
}

fn validate_context(
    app_id: &str,
    stage_id: &str,
    manifest_hash: &str,
    updated_at: &str,
    release_ref: &str,
    invalid: fn(&str, String) -> Error,
) -> Result<String> {
    for (field, value) in [("appId", app_id), ("stageId", stage_id)] {
        validation::slug(value).map_err(|message| invalid(field, message.to_owned()))?;
    }
    validation::manifest_hash(manifest_hash)
        .map_err(|message| invalid("expectedManifestHash", message.to_owned()))?;
    validation::utc_timestamp(updated_at)
        .map_err(|message| invalid("expectedUpdatedAt", message))?;
    validation::normalize_ref(release_ref).map_err(|message| invalid("releaseRef", message))
}

fn stage<'a>(app: &'a RegisteredApp, stage_id: &str) -> Result<&'a StageSummary> {
    app.manifest
        .stages
        .iter()
        .find(|stage| stage.id == stage_id)
        .ok_or_else(|| Error::StageNotFound {
            app_id: app.manifest.app_id.clone(),
            stage_id: stage_id.to_owned(),
        })
}

fn matches_harness(
    result: &HarnessResult,
    assessment: &PromotionAssessment,
    check_id: &str,
) -> bool {
    result.app_id == assessment.app_id
        && result.app_name == assessment.app_name
        && result.stage_id == assessment.stage_id
        && result.check_id == check_id
        && result.release_ref == assessment.release_ref
        && result.manifest_hash == assessment.manifest_hash
        && result.manifest_path == assessment.manifest_path
        && result.source_path == assessment.source_path
        && result.registration_updated_at == assessment.registration_updated_at
}

fn matches_deployment(
    deployment: &ConsumerDeployment,
    assessment: &PromotionAssessment,
    consumer: &ConsumerSummary,
) -> bool {
    deployment.app_id == assessment.app_id
        && deployment.app_name == assessment.app_name
        && deployment.stage_id == assessment.stage_id
        && deployment.consumer == *consumer
        && deployment.release_ref == assessment.release_ref
        && deployment.manifest_hash == assessment.manifest_hash
        && deployment.manifest_path == assessment.manifest_path
        && deployment.source_path == assessment.source_path
        && deployment.registration_updated_at == assessment.registration_updated_at
}

fn evaluate_criterion(
    criterion: &PromotionCriterion,
    evidence_id: Option<String>,
    observed: Option<HarnessCounts>,
    consumer: Option<ConsumerSummary>,
) -> std::result::Result<CriterionEvaluation, String> {
    if let Some(id) = &evidence_id {
        validation::insertion_id(id)?;
    }
    let (status, message) = match criterion {
        PromotionCriterion::Harness {
            check_id,
            expected_tests,
            minimum_passed,
            ..
        } => {
            if consumer.is_some() || evidence_id.is_some() != observed.is_some() {
                return Err(
                    "harness evidence must contain both an ID and counts, and no consumer"
                        .to_owned(),
                );
            }
            let requirement =
                format!("exactly {expected_tests} tests and at least {minimum_passed} passed");
            match observed {
                Some(counts) => {
                    validation::harness_counts(counts.total, counts.passed)?;
                    let satisfied =
                        counts.total == *expected_tests && counts.passed >= *minimum_passed;
                    (
                        if satisfied {
                            CriterionStatus::Satisfied
                        } else {
                            CriterionStatus::Blocked
                        },
                        format!(
                            "Latest external report for check '{check_id}': {} passed out of {} total; requires {requirement}. {}",
                            counts.passed,
                            counts.total,
                            if satisfied {
                                "Declared criterion satisfied; execution was not verified."
                            } else {
                                "Declared criterion is blocked."
                            },
                        ),
                    )
                }
                None => (
                    CriterionStatus::Blocked,
                    format!(
                        "No external result for check '{check_id}' matches this candidate and registration; requires {requirement}."
                    ),
                ),
            }
        }
        PromotionCriterion::Deployment {
            consumer_id,
            requirement,
            ..
        } => {
            if observed.is_some() {
                return Err("deployment criteria must not contain harness counts".to_owned());
            }
            let destination = consumer.as_ref().ok_or_else(|| {
                "deployment criteria must include their frozen consumer descriptor".to_owned()
            })?;
            if destination.id != *consumer_id {
                return Err("consumer descriptor does not match the criterion".to_owned());
            }
            validation::slug(&destination.id)?;
            validation::display_text(&destination.name, validation::CONSUMER_NAME_LIMIT)?;
            validation::display_text(&destination.target, validation::CONSUMER_TARGET_LIMIT)?;
            if evidence_id.is_some() {
                (
                    CriterionStatus::Satisfied,
                    format!(
                        "Manual deployment marker for '{}' at '{}' matches this candidate and registration; no deployment was performed by this assessment.",
                        destination.name, destination.target
                    ),
                )
            } else {
                match requirement {
                    DeploymentRequirement::Required => (
                        CriterionStatus::Blocked,
                        format!(
                            "Required deployment to '{}' at '{}' has no marker matching this candidate and registration.",
                            destination.name, destination.target
                        ),
                    ),
                    DeploymentRequirement::Optional => (
                        CriterionStatus::Warning,
                        format!(
                            "Optional deployment to '{}' at '{}' has no marker matching this candidate and registration; explicitly acknowledge skipping this criterion to approve.",
                            destination.name, destination.target
                        ),
                    ),
                }
            }
        }
    };
    Ok(CriterionEvaluation {
        criterion: criterion.clone(),
        status,
        message,
        evidence_id,
        observed,
        consumer,
    })
}

fn assessment_status(criteria: &[CriterionEvaluation]) -> PromotionStatus {
    if criteria.is_empty() {
        PromotionStatus::NotConfigured
    } else if criteria
        .iter()
        .any(|criterion| criterion.status == CriterionStatus::Blocked)
    {
        PromotionStatus::Blocked
    } else if criteria
        .iter()
        .any(|criterion| criterion.status == CriterionStatus::Warning)
    {
        PromotionStatus::ReadyWithWarnings
    } else {
        PromotionStatus::Ready
    }
}

fn skipped_optional(assessment: &PromotionAssessment) -> Vec<String> {
    assessment
        .criteria
        .iter()
        .filter(|evaluation| evaluation.status == CriterionStatus::Warning)
        .map(|evaluation| evaluation.criterion.id().to_owned())
        .collect()
}

fn assessment_hash(assessment: &PromotionAssessment) -> serde_json::Result<String> {
    let mut value = serde_json::to_value(assessment)?;
    if let serde_json::Value::Object(fields) = &mut value {
        fields.remove("assessmentHash");
    }
    sort_keys(&mut value);
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&value)?)))
}

fn sort_keys(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            fields.sort_keys();
            for value in fields.values_mut() {
                sort_keys(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                sort_keys(value);
            }
        }
        _ => {}
    }
}
