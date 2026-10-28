//! Shared manifests, registrations, attached pipeline execution, and durable history.
//!
//! A preview validates planned configuration, not whether a release would succeed.
//! Registration stores that exact YAML snapshot; neither operation runs commands,
//! reads version files, accesses credentials, or checks Git state. Manual consumer
//! approvals only append durable history; they never deploy anything. Promotion
//! assessments evaluate declared criteria against external reports and markers.
//! The explicitly approved runner enforces execution prerequisites separately,
//! and observes current policy approvals before releasing promotion gates.

mod error;
mod legacy;
mod manifest;
mod model;
mod registry;
mod results;
mod runner;
mod validation;

pub use error::{Error, Result};
pub use manifest::preview_manifest;
pub use model::{
    ApprovePromotion, CheckPlanNode, CommandSummary, ConsumerApproval, ConsumerDeployment,
    ConsumerSummary, CriterionEvaluation, CriterionStatus, DeploymentRequirement, ExecutionSummary,
    HarnessCounts, HarnessResult, ManifestPreview, OutputSummary, Parallelism, PromotionApproval,
    PromotionAssessment, PromotionContext, PromotionCriterion, PromotionStatus,
    RecordConsumerDeployment, RecordHarnessResult, RegisteredApp, StageSummary,
};
pub use model::{
    CommandResult, CommandRun, LogStream, PipelineRun, PipelineRunSummary, RunLogEntry, RunLogPage,
    RunStatus, StageRun, StartPipelineRun,
};
pub use registry::{Registry, default_data_dir};
pub use runner::PipelineRunner;
