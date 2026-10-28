//! Frozen schema-v1/v2/v3/v4 DTOs. Current snapshots never default missing metadata.

use serde::Deserialize;

use crate::{
    CheckPlanNode, CommandSummary, ConsumerSummary, ExecutionSummary, ManifestPreview,
    OutputSummary, Parallelism, StageSummary,
};

pub(crate) type ManifestPreviewV1 = LegacyManifest<StageSummaryV1, LegacyExecution>;
pub(crate) type ManifestPreviewV2 = LegacyManifest<StageSummaryV2, LegacyExecution>;
pub(crate) type ManifestPreviewV3 = LegacyManifest<StageSummaryV3, ExecutionSummary>;
pub(crate) type ManifestPreviewV4 = LegacyManifest<StageSummaryV4, ExecutionSummary>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LegacyManifest<S, E> {
    app_id: String,
    name: String,
    manifest_path: String,
    source_path: String,
    manifest_hash: String,
    manifest_yaml: String,
    version_from: String,
    execution: E,
    stages: Vec<S>,
    prepare: Vec<CommandSummary>,
    outputs: Vec<OutputSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StageSummaryV1 {
    id: String,
    checks: Vec<CommandSummary>,
    requires: Vec<String>,
    requires_approval: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StageSummaryV2 {
    id: String,
    after: Vec<String>,
    checks: Vec<CommandSummary>,
    requires: Vec<String>,
    requires_approval: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StageSummaryV3 {
    id: String,
    after: Vec<String>,
    checks: Vec<CommandSummary>,
    check_plan: Vec<CheckPlanNode>,
    requires: Vec<String>,
    requires_approval: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StageSummaryV4 {
    id: String,
    after: Vec<String>,
    checks: Vec<CommandSummary>,
    check_plan: Vec<CheckPlanNode>,
    requires: Vec<String>,
    requires_approval: bool,
    consumers: Vec<ConsumerSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LegacyExecution {
    os: Option<String>,
    arch: Option<String>,
    timeout: Option<String>,
}

impl LegacyManifest<StageSummaryV1, LegacyExecution> {
    pub(crate) fn into_current(self) -> ManifestPreview {
        self.upgrade(|stages| {
            let mut previous = None;
            stages
                .into_iter()
                .map(|stage| {
                    let after = previous.replace(stage.id.clone()).into_iter().collect();
                    Self::upgrade_stage(
                        stage.id,
                        after,
                        stage.checks,
                        stage.requires,
                        stage.requires_approval,
                    )
                })
                .collect()
        })
    }
}

impl LegacyManifest<StageSummaryV2, LegacyExecution> {
    pub(crate) fn into_current(self) -> ManifestPreview {
        self.upgrade(|stages| {
            stages
                .into_iter()
                .map(|stage| {
                    Self::upgrade_stage(
                        stage.id,
                        stage.after,
                        stage.checks,
                        stage.requires,
                        stage.requires_approval,
                    )
                })
                .collect()
        })
    }
}

impl LegacyManifest<StageSummaryV3, ExecutionSummary> {
    pub(crate) fn into_current(self) -> ManifestPreview {
        self.upgrade(|stages| {
            stages
                .into_iter()
                .map(|stage| StageSummary {
                    id: stage.id,
                    after: stage.after,
                    checks: stage.checks,
                    check_plan: stage.check_plan,
                    requires: stage.requires,
                    requires_approval: stage.requires_approval,
                    consumers: Vec::new(),
                    promotion_criteria: Vec::new(),
                })
                .collect()
        })
    }
}

impl LegacyManifest<StageSummaryV4, ExecutionSummary> {
    pub(crate) fn into_current(self) -> ManifestPreview {
        self.upgrade(|stages| {
            stages
                .into_iter()
                .map(|stage| StageSummary {
                    id: stage.id,
                    after: stage.after,
                    checks: stage.checks,
                    check_plan: stage.check_plan,
                    requires: stage.requires,
                    requires_approval: stage.requires_approval,
                    consumers: stage.consumers,
                    promotion_criteria: Vec::new(),
                })
                .collect()
        })
    }
}

impl From<LegacyExecution> for ExecutionSummary {
    fn from(execution: LegacyExecution) -> Self {
        Self {
            os: execution.os,
            arch: execution.arch,
            timeout: execution.timeout,
            max_parallel: Parallelism::Auto,
        }
    }
}

impl<S, E: Into<ExecutionSummary>> LegacyManifest<S, E> {
    fn upgrade(self, stages: impl FnOnce(Vec<S>) -> Vec<StageSummary>) -> ManifestPreview {
        ManifestPreview {
            app_id: self.app_id,
            name: self.name,
            manifest_path: self.manifest_path,
            source_path: self.source_path,
            manifest_hash: self.manifest_hash,
            manifest_yaml: self.manifest_yaml,
            version_from: self.version_from,
            execution: self.execution.into(),
            stages: stages(self.stages),
            prepare: self.prepare,
            outputs: self.outputs,
        }
    }

    fn upgrade_stage(
        id: String,
        after: Vec<String>,
        checks: Vec<CommandSummary>,
        requires: Vec<String>,
        requires_approval: bool,
    ) -> StageSummary {
        StageSummary {
            id,
            after,
            check_plan: checks
                .iter()
                .cloned()
                .map(|command| CheckPlanNode::Command { command })
                .collect(),
            checks,
            requires,
            requires_approval,
            consumers: Vec::new(),
            promotion_criteria: Vec::new(),
        }
    }
}
