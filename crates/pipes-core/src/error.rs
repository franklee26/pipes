use std::{io, path::PathBuf};

/// A failure with a stable machine-readable code and actionable human context.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid manifest '{}', {field}: {message}", path.display())]
    InvalidManifest {
        path: PathBuf,
        field: String,
        message: String,
    },
    #[error("Cannot read manifest '{}': {source}", path.display())]
    ManifestIo {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Source directory '{}' does not exist; update source.path or create the directory", path.display())]
    SourceNotFound { path: PathBuf },
    #[error("Cannot access source directory '{}': {source}", path.display())]
    SourceIo {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Invalid data directory: {message}")]
    InvalidDataDir { message: String },
    #[error("App '{app_id}' is already registered: {message}")]
    AlreadyRegistered { app_id: String, message: String },
    #[error("Manifest '{}' changed since preview (expected {expected_hash}, found {actual_hash}); preview it again before registering", path.display())]
    ManifestChanged {
        path: PathBuf,
        expected_hash: String,
        actual_hash: String,
    },
    #[error("App '{app_id}' is not registered; use list to see registered app IDs")]
    NotFound { app_id: String },
    #[error("Explicit approval is required to execute the reviewed pipeline commands")]
    RunApprovalRequired,
    #[error("Invalid pipeline run: {message}")]
    InvalidRun { message: String },
    #[error("Pipeline run '{run_id}' does not exist")]
    RunNotFound { run_id: String },
    #[error("Pipeline run conflict: {message}")]
    RunConflict { message: String },
    #[error("Pipeline execution failed: {message}")]
    Execution { message: String },
    #[error(
        "Explicit manual approval is required to record a consumer deployment or approve declared promotion criteria; no deployment or stage advancement is performed"
    )]
    ApprovalRequired,
    #[error("Invalid consumer deployment, {field}: {message}")]
    InvalidDeployment { field: String, message: String },
    #[error("Invalid harness result, {field}: {message}")]
    InvalidHarnessResult { field: String, message: String },
    #[error("Invalid promotion, {field}: {message}")]
    InvalidPromotion { field: String, message: String },
    #[error("Stage '{stage_id}' is not declared in app '{app_id}'")]
    StageNotFound { app_id: String, stage_id: String },
    #[error("Check '{check_id}' is not a command leaf in stage '{stage_id}' of app '{app_id}'")]
    CheckNotFound {
        app_id: String,
        stage_id: String,
        check_id: String,
    },
    #[error("Stage '{stage_id}' of app '{app_id}' has no declared promotion criteria")]
    PromotionNotConfigured { app_id: String, stage_id: String },
    #[error("Declared promotion criteria for stage '{stage_id}' of app '{app_id}' are blocked")]
    PromotionBlocked { app_id: String, stage_id: String },
    #[error("Explicit acknowledgement is required to skip outstanding optional promotion criteria")]
    OptionalStepsUnacknowledged,
    #[error(
        "Promotion evidence changed since review; assess and review the declared criteria again"
    )]
    AssessmentChanged,
    #[error(
        "App '{app_id}' changed since review; reload its registered manifest and review the action again before approving"
    )]
    RegistrationChanged { app_id: String },
    #[error(
        "Consumer '{consumer_id}' is not declared in stage '{stage_id}' of app '{app_id}'; refresh the registered manifest and choose an existing stage and consumer"
    )]
    ConsumerNotFound {
        app_id: String,
        stage_id: String,
        consumer_id: String,
    },
    #[error("Registry storage error at '{}': {message}", path.display())]
    Storage { path: PathBuf, message: String },
    #[error("Registry '{}' has unsupported schema version {found}; this version of Pipes supports {supported}. Use a compatible Pipes version; the registry has not been reset", path.display())]
    UnsupportedSchema {
        path: PathBuf,
        found: i64,
        supported: i64,
    },
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidManifest { .. } => "invalid_manifest",
            Self::ManifestIo { .. } => "manifest_io",
            Self::SourceNotFound { .. } => "source_not_found",
            Self::SourceIo { .. } => "source_io",
            Self::InvalidDataDir { .. } => "invalid_data_dir",
            Self::AlreadyRegistered { .. } => "already_registered",
            Self::ManifestChanged { .. } => "manifest_changed",
            Self::NotFound { .. } => "not_found",
            Self::RunApprovalRequired => "run_approval_required",
            Self::InvalidRun { .. } => "invalid_run",
            Self::RunNotFound { .. } => "run_not_found",
            Self::RunConflict { .. } => "run_conflict",
            Self::Execution { .. } => "execution",
            Self::ApprovalRequired => "approval_required",
            Self::InvalidDeployment { .. } => "invalid_deployment",
            Self::InvalidHarnessResult { .. } => "invalid_harness_result",
            Self::InvalidPromotion { .. } => "invalid_promotion",
            Self::StageNotFound { .. } => "stage_not_found",
            Self::CheckNotFound { .. } => "check_not_found",
            Self::PromotionNotConfigured { .. } => "promotion_not_configured",
            Self::PromotionBlocked { .. } => "promotion_blocked",
            Self::OptionalStepsUnacknowledged => "optional_steps_unacknowledged",
            Self::AssessmentChanged => "assessment_changed",
            Self::RegistrationChanged { .. } => "registration_changed",
            Self::ConsumerNotFound { .. } => "consumer_not_found",
            Self::Storage { .. } => "storage",
            Self::UnsupportedSchema { .. } => "unsupported_schema",
        }
    }

    pub(crate) fn invalid(
        path: impl Into<PathBuf>,
        field: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::InvalidManifest {
            path: path.into(),
            field: field.into(),
            message: message.into(),
        }
    }

    pub(crate) fn storage(path: impl Into<PathBuf>, message: impl std::fmt::Display) -> Self {
        Self::Storage {
            path: path.into(),
            message: message.to_string(),
        }
    }

    pub(crate) fn execution(message: impl std::fmt::Display) -> Self {
        Self::Execution {
            message: message.to_string(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
