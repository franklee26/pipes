use serde::{Deserialize, Deserializer, Serialize};

use super::ConsumerSummary;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PromotionCriterion {
    Harness {
        id: String,
        check_id: String,
        expected_tests: u32,
        minimum_passed: u32,
    },
    Deployment {
        id: String,
        consumer_id: String,
        requirement: DeploymentRequirement,
    },
}

impl PromotionCriterion {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Harness { id, .. } | Self::Deployment { id, .. } => id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", try_from = "String", deny_unknown_fields)]
pub enum DeploymentRequirement {
    Required,
    Optional,
}

impl TryFrom<String> for DeploymentRequirement {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "required" => Ok(Self::Required),
            "optional" => Ok(Self::Optional),
            _ => Err("deployment requirement must be 'required' or 'optional'".to_owned()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", try_from = "String", deny_unknown_fields)]
pub enum CriterionStatus {
    Satisfied,
    Blocked,
    Warning,
}

impl TryFrom<String> for CriterionStatus {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "satisfied" => Ok(Self::Satisfied),
            "blocked" => Ok(Self::Blocked),
            "warning" => Ok(Self::Warning),
            _ => Err("criterion status must be 'satisfied', 'blocked', or 'warning'".to_owned()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", try_from = "String", deny_unknown_fields)]
pub enum PromotionStatus {
    NotConfigured,
    Blocked,
    Ready,
    ReadyWithWarnings,
}

impl TryFrom<String> for PromotionStatus {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "not_configured" => Ok(Self::NotConfigured),
            "blocked" => Ok(Self::Blocked),
            "ready" => Ok(Self::Ready),
            "ready_with_warnings" => Ok(Self::ReadyWithWarnings),
            _ => Err(
                "promotion status must be 'not_configured', 'blocked', 'ready', or 'ready_with_warnings'"
                    .to_owned(),
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromotionContext {
    pub app_id: String,
    pub stage_id: String,
    pub expected_manifest_hash: String,
    pub expected_updated_at: String,
    pub release_ref: String,
}

/// Caller-reported external results, never a request to execute or verify a check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordHarnessResult {
    pub app_id: String,
    pub stage_id: String,
    pub expected_manifest_hash: String,
    pub expected_updated_at: String,
    pub release_ref: String,
    pub check_id: String,
    pub total: u32,
    pub passed: u32,
    pub notes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessResult {
    pub id: String,
    pub app_id: String,
    pub app_name: String,
    pub stage_id: String,
    pub check_id: String,
    pub release_ref: String,
    pub total: u32,
    pub passed: u32,
    #[serde(deserialize_with = "required_nullable")]
    pub notes: Option<String>,
    pub manifest_hash: String,
    pub manifest_path: String,
    pub source_path: String,
    pub registration_updated_at: String,
    pub recorded_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessCounts {
    pub total: u32,
    pub passed: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CriterionEvaluation {
    pub criterion: PromotionCriterion,
    pub status: CriterionStatus,
    pub message: String,
    #[serde(deserialize_with = "required_nullable")]
    pub evidence_id: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub observed: Option<HarnessCounts>,
    #[serde(deserialize_with = "required_nullable")]
    pub consumer: Option<ConsumerSummary>,
}

/// Readiness of declared criteria only. Stage requirements are not evaluated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromotionAssessment {
    pub app_id: String,
    pub app_name: String,
    pub stage_id: String,
    pub release_ref: String,
    pub manifest_hash: String,
    pub manifest_path: String,
    pub source_path: String,
    pub registration_updated_at: String,
    pub status: PromotionStatus,
    pub criteria: Vec<CriterionEvaluation>,
    pub stage_requirements: Vec<String>,
    pub assessment_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovePromotion {
    pub app_id: String,
    pub stage_id: String,
    pub expected_manifest_hash: String,
    pub expected_updated_at: String,
    pub release_ref: String,
    pub expected_assessment_hash: String,
    pub approved: bool,
    pub acknowledge_optional: bool,
    pub notes: Option<String>,
}

/// Immutable acknowledgement of a frozen assessment, never stage advancement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromotionApproval {
    pub id: String,
    pub assessment: PromotionAssessment,
    pub skipped_optional: Vec<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub notes: Option<String>,
    pub recorded_at: String,
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}
