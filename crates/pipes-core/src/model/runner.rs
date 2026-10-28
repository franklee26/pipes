use serde::{Deserialize, Serialize};

use super::ManifestPreview;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    WaitingApproval,
    Passed,
    Failed,
    Cancelled,
    Interrupted,
    Blocked,
    Skipped,
    TimedOut,
}

impl RunStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running | Self::WaitingApproval)
    }

    pub fn is_success(self) -> bool {
        self == Self::Passed
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartPipelineRun {
    pub app_id: String,
    pub expected_manifest_hash: String,
    pub expected_updated_at: String,
    pub release_ref: String,
    pub approved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageRun {
    pub stage_id: String,
    pub status: RunStatus,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub approved_at: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandRun {
    pub stage_id: Option<String>,
    pub command_id: String,
    pub status: RunStatus,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PipelineRun {
    pub id: String,
    pub app_id: String,
    pub app_name: String,
    pub release_ref: String,
    pub manifest: ManifestPreview,
    pub registration_updated_at: String,
    pub status: RunStatus,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub max_parallel: u32,
    pub message: Option<String>,
    pub stages: Vec<StageRun>,
    pub commands: Vec<CommandRun>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PipelineRunSummary {
    pub id: String,
    pub app_id: String,
    pub app_name: String,
    pub release_ref: String,
    pub manifest_hash: String,
    pub registration_updated_at: String,
    pub status: RunStatus,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub message: Option<String>,
    pub stages: Vec<StageRun>,
    pub total_commands: usize,
    pub passed_commands: usize,
}

impl PipelineRun {
    pub fn summary(&self) -> PipelineRunSummary {
        PipelineRunSummary {
            id: self.id.clone(),
            app_id: self.app_id.clone(),
            app_name: self.app_name.clone(),
            release_ref: self.release_ref.clone(),
            manifest_hash: self.manifest.manifest_hash.clone(),
            registration_updated_at: self.registration_updated_at.clone(),
            status: self.status,
            started_at: self.started_at.clone(),
            finished_at: self.finished_at.clone(),
            message: self.message.clone(),
            stages: self.stages.clone(),
            total_commands: self.commands.len(),
            passed_commands: self
                .commands
                .iter()
                .filter(|command| command.status.is_success())
                .count(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunLogEntry {
    pub sequence: u64,
    pub recorded_at: String,
    pub stage_id: Option<String>,
    pub command_id: Option<String>,
    pub stream: LogStream,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunLogPage {
    pub entries: Vec<RunLogEntry>,
    pub next_cursor: u64,
    pub has_more: bool,
}
