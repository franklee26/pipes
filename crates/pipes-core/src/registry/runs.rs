use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io,
    path::Path,
};

use chrono::{SecondsFormat, Utc};
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use super::{Registry, create_private_dir, create_private_file};
use crate::{
    CommandRun, Error, LogStream, PipelineRun, PipelineRunSummary, PromotionContext,
    PromotionStatus, Result, RunLogEntry, RunLogPage, RunStatus, StageRun, StartPipelineRun,
    manifest::{SnapshotVersion, validate_snapshot},
    validation,
};

const SELECT_RUN: &str = "SELECT id, app_id, source_path, snapshot, active FROM pipeline_runs";
const LOG_PAGE_SIZE: usize = 256;
const MAX_LOG_CURSOR: u64 = 9_007_199_254_740_991;

pub(crate) fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true)
}

pub(crate) struct ExecutionLock {
    _file: File,
}

pub(crate) struct RunControl {
    pub cancelled: bool,
    pub approvals: BTreeMap<String, String>,
}

pub(crate) struct RunWriter {
    registry: Registry,
    connection: Connection,
}

impl Registry {
    pub fn pipeline_runs(&self, app_id: Option<&str>) -> Result<Vec<PipelineRunSummary>> {
        if let Some(app_id) = app_id {
            validation::slug(app_id).map_err(invalid_run)?;
        }
        self.recover_interrupted_runs()?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(&format!(
                "{SELECT_RUN} WHERE (?1 IS NULL OR app_id = ?1) ORDER BY id DESC LIMIT 100"
            ))
            .map_err(|error| self.storage("list pipeline runs", error))?;
        let rows = statement
            .query_map([app_id], StoredRun::read)
            .map_err(|error| self.storage("list pipeline runs", error))?;
        rows.map(|row| {
            row.map_err(|error| self.storage("read pipeline run", error))?
                .decode(self)
                .map(|run| run.summary())
        })
        .collect()
    }

    pub fn pipeline_run(&self, run_id: &str) -> Result<PipelineRun> {
        validate_id(run_id)?;
        self.recover_interrupted_runs()?;
        self.load_pipeline_run(&self.connection()?, run_id)
    }

    pub fn pipeline_run_logs(&self, run_id: &str, after: u64) -> Result<RunLogPage> {
        validate_id(run_id)?;
        if after > MAX_LOG_CURSOR {
            return Err(invalid_run(
                "log cursor exceeds the exact JSON integer range",
            ));
        }
        self.recover_interrupted_runs()?;
        let after = i64::try_from(after).map_err(|_| invalid_run("log cursor is too large"))?;
        let connection = self.connection()?;
        let run = self.load_pipeline_run(&connection, run_id)?;
        let mut statement = connection
            .prepare(
                "SELECT id, recorded_at, stage_id, command_id, stream, text
             FROM pipeline_run_logs WHERE run_id = ?1 AND id > ?2 ORDER BY id LIMIT ?3",
            )
            .map_err(|error| self.storage("read pipeline output", error))?;
        let rows = statement
            .query_map(params![run_id, after, LOG_PAGE_SIZE + 1], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(|error| self.storage("read pipeline output", error))?;
        let mut entries = Vec::new();
        for row in rows {
            let (sequence, recorded_at, stage_id, command_id, stream, text) =
                row.map_err(|error| self.storage("decode pipeline output", error))?;
            if sequence == 0 || sequence > MAX_LOG_CURSOR {
                return Err(self.storage(
                    "decode pipeline output",
                    "log sequence exceeds the exact JSON cursor range",
                ));
            }
            let stream = match stream.as_str() {
                "stdout" => LogStream::Stdout,
                "stderr" => LogStream::Stderr,
                "system" => LogStream::System,
                _ => return Err(self.storage("decode pipeline output", "unknown output stream")),
            };
            if stage_id
                .as_ref()
                .is_some_and(|id| !run.stages.iter().any(|stage| stage.stage_id == *id))
                || command_id.as_ref().is_some_and(|id| {
                    !run.commands
                        .iter()
                        .any(|command| command.command_id == *id && command.stage_id == stage_id)
                })
                || (stream != LogStream::System && command_id.is_none())
            {
                return Err(self.storage(
                    "decode pipeline output",
                    "output attribution does not match the saved run",
                ));
            }
            validation::utc_timestamp(&recorded_at)
                .map_err(|error| self.storage("decode pipeline output timestamp", error))?;
            entries.push(RunLogEntry {
                sequence,
                recorded_at,
                stage_id,
                command_id,
                stream,
                text,
            });
        }
        let has_more = entries.len() > LOG_PAGE_SIZE;
        entries.truncate(LOG_PAGE_SIZE);
        Ok(RunLogPage {
            next_cursor: entries.last().map_or(after as u64, |entry| entry.sequence),
            entries,
            has_more,
        })
    }

    pub fn cancel_pipeline_run(&self, run_id: &str) -> Result<PipelineRun> {
        validate_id(run_id)?;
        self.recover_interrupted_runs()?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| self.storage("begin run cancellation", error))?;
        self.require_version(&transaction)?;
        self.validate_schema(&transaction)?;
        let mut run = self.load_pipeline_run(&transaction, run_id)?;
        if !run.status.is_active() {
            return Err(conflict("the run has already finished"));
        }
        transaction
            .execute(
                "UPDATE pipeline_runs SET cancel_requested = 1 WHERE id = ?1",
                [run_id],
            )
            .map_err(|error| self.storage("request run cancellation", error))?;
        transaction
            .commit()
            .map_err(|error| self.storage("commit run cancellation", error))?;
        run.message = Some("Cancellation requested; stopping running commands.".to_owned());
        Ok(run)
    }

    pub fn approve_pipeline_stage(
        &self,
        run_id: &str,
        stage_id: &str,
        approved: bool,
    ) -> Result<PipelineRun> {
        if !approved {
            return Err(Error::RunApprovalRequired);
        }
        validate_id(run_id)?;
        validation::slug(stage_id).map_err(invalid_run)?;
        self.recover_interrupted_runs()?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| self.storage("begin stage execution approval", error))?;
        self.require_version(&transaction)?;
        self.validate_schema(&transaction)?;
        let mut run = self.load_pipeline_run(&transaction, run_id)?;
        if !run.status.is_active() {
            return Err(conflict("the run has already finished"));
        }
        let index = run
            .stages
            .iter()
            .position(|stage| stage.stage_id == stage_id)
            .ok_or_else(|| Error::StageNotFound {
                app_id: run.app_id.clone(),
                stage_id: stage_id.to_owned(),
            })?;
        if !run.manifest.stages[index].requires_approval
            || run.stages[index].status != RunStatus::WaitingApproval
            || run.stages[index].approved_at.is_some()
        {
            return Err(conflict(
                "this stage is not waiting for local execution approval; custom promotion criteria use approve-promotion",
            ));
        }
        let mut control = read_control(self, &transaction, run_id)?;
        if control.cancelled || control.approvals.contains_key(stage_id) {
            return Err(conflict(
                "the run was cancelled or the stage was already approved",
            ));
        }
        let approved_at = now();
        control
            .approvals
            .insert(stage_id.to_owned(), approved_at.clone());
        let approvals = serde_json::to_string(&control.approvals)
            .map_err(|error| self.storage("encode stage execution approval", error))?;
        transaction
            .execute(
                "UPDATE pipeline_runs SET stage_approvals = ?1 WHERE id = ?2",
                params![approvals, run_id],
            )
            .map_err(|error| self.storage("record stage execution approval", error))?;
        insert_logs(
            self,
            &transaction,
            run_id,
            &[RunLogEntry {
                sequence: 0,
                recorded_at: approved_at.clone(),
                stage_id: Some(stage_id.to_owned()),
                command_id: None,
                stream: LogStream::System,
                text: "Local stage execution explicitly approved.\n".to_owned(),
            }],
        )?;
        transaction
            .commit()
            .map_err(|error| self.storage("commit stage execution approval", error))?;
        run.stages[index].approved_at = Some(approved_at);
        Ok(run)
    }

    pub(crate) fn try_source_lock(&self, source: &str) -> Result<Option<ExecutionLock>> {
        self.try_execution_lock("source", source)
    }

    pub(crate) fn try_result_lock(&self, path: &Path) -> Result<Option<ExecutionLock>> {
        let path = path
            .to_str()
            .ok_or_else(|| Error::execution("result path must be UTF-8"))?;
        self.try_execution_lock("result", path)
    }

    fn try_execution_lock(&self, namespace: &str, resource: &str) -> Result<Option<ExecutionLock>> {
        let directory = self.data_dir.join("run-locks");
        create_private_dir(&directory)
            .map_err(|error| self.storage("create runner lock directory", error))?;
        let path = directory.join(format!(
            "{namespace}-{:x}.lock",
            Sha256::digest(resource.as_bytes())
        ));
        create_private_file(&path)
            .map_err(|error| self.storage("create source execution lock", error))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| self.storage("open source execution lock", error))?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Some(ExecutionLock { _file: file })),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(self.storage("lock source execution", error)),
        }
    }

    pub(crate) fn create_pipeline_run(
        &self,
        request: &StartPipelineRun,
        max_parallel: u32,
        locked_source: &str,
    ) -> Result<PipelineRun> {
        if !request.approved {
            return Err(Error::RunApprovalRequired);
        }
        validation::slug(&request.app_id).map_err(invalid_run)?;
        validation::manifest_hash(&request.expected_manifest_hash).map_err(invalid_run)?;
        validation::utc_timestamp(&request.expected_updated_at).map_err(invalid_run)?;
        let release_ref = validation::normalize_ref(&request.release_ref).map_err(invalid_run)?;
        self.interrupt_source_runs(locked_source)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| self.storage("begin pipeline run", error))?;
        self.require_version(&transaction)?;
        self.validate_schema(&transaction)?;
        let app = self.reviewed_app(
            &transaction,
            &request.app_id,
            &request.expected_manifest_hash,
            &request.expected_updated_at,
        )?;
        if app.manifest.source_path != locked_source {
            return Err(Error::RegistrationChanged {
                app_id: request.app_id.clone(),
            });
        }
        let stages = app
            .manifest
            .stages
            .iter()
            .map(|stage| StageRun {
                stage_id: stage.id.clone(),
                status: RunStatus::Pending,
                started_at: None,
                finished_at: None,
                approved_at: None,
                message: None,
            })
            .collect();
        let commands = app
            .manifest
            .prepare
            .iter()
            .map(|command| (None, command))
            .chain(app.manifest.stages.iter().flat_map(|stage| {
                stage
                    .checks
                    .iter()
                    .map(move |command| (Some(stage.id.clone()), command))
            }))
            .map(|(stage_id, command)| CommandRun {
                stage_id,
                command_id: command.id.clone(),
                status: RunStatus::Pending,
                started_at: None,
                finished_at: None,
                exit_code: None,
                message: None,
            })
            .collect();
        let mut run = PipelineRun {
            id: String::new(),
            app_id: app.manifest.app_id.clone(),
            app_name: app.manifest.name.clone(),
            release_ref,
            manifest: app.manifest,
            registration_updated_at: app.updated_at,
            status: RunStatus::Pending,
            started_at: now(),
            finished_at: None,
            max_parallel,
            message: None,
            stages,
            commands,
        };
        transaction.execute(
            "INSERT INTO pipeline_runs (app_id, source_path, snapshot, active, cancel_requested, stage_approvals)
             VALUES (?1, ?2, '', 1, 0, '{}')",
            params![run.app_id, locked_source],
        ).map_err(|error| self.storage("allocate pipeline run", error))?;
        run.id = transaction.last_insert_rowid().to_string();
        let snapshot = serde_json::to_string(&run)
            .map_err(|error| self.storage("encode pipeline run", error))?;
        transaction
            .execute(
                "UPDATE pipeline_runs SET snapshot = ?1 WHERE id = ?2",
                params![snapshot, run.id],
            )
            .map_err(|error| self.storage("save pipeline run", error))?;
        insert_logs(
            self,
            &transaction,
            &run.id,
            &[RunLogEntry {
                sequence: 0,
                recorded_at: run.started_at.clone(),
                stage_id: None,
                command_id: None,
                stream: LogStream::System,
                text: format!(
                    "Run {} approved for {} at {}. Executing the live working tree; no source snapshot or artifact attestation.\n",
                    run.id, run.release_ref, locked_source
                ),
            }],
        )?;
        transaction
            .commit()
            .map_err(|error| self.storage("commit pipeline run", error))?;
        Ok(run)
    }

    pub(crate) fn run_writer(&self) -> Result<RunWriter> {
        Ok(RunWriter {
            registry: self.clone(),
            connection: self.connection()?,
        })
    }

    fn load_pipeline_run(&self, connection: &Connection, run_id: &str) -> Result<PipelineRun> {
        connection
            .query_row(
                &format!("{SELECT_RUN} WHERE id = ?1"),
                [run_id],
                StoredRun::read,
            )
            .optional()
            .map_err(|error| self.storage("read pipeline run", error))?
            .ok_or_else(|| Error::RunNotFound {
                run_id: run_id.to_owned(),
            })?
            .decode(self)
    }

    fn active_runs(&self, source: Option<&str>) -> Result<Vec<PipelineRun>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(&format!(
                "{SELECT_RUN} WHERE active = 1 AND (?1 IS NULL OR source_path = ?1)"
            ))
            .map_err(|error| self.storage("read active runs", error))?;
        let rows = statement
            .query_map([source], StoredRun::read)
            .map_err(|error| self.storage("read active runs", error))?;
        rows.map(|row| {
            row.map_err(|error| self.storage("read active run", error))?
                .decode(self)
        })
        .collect()
    }

    fn recover_interrupted_runs(&self) -> Result<()> {
        for run in self.active_runs(None)? {
            if let Some(_lock) = self.try_source_lock(&run.manifest.source_path)? {
                self.interrupt_source_runs(&run.manifest.source_path)?;
            }
        }
        Ok(())
    }

    fn interrupt_source_runs(&self, source: &str) -> Result<()> {
        let mut writer = self.run_writer()?;
        for mut run in self.active_runs(Some(source))? {
            interrupt(
                &mut run,
                "The owning process ended before this run finished. Commands were not resumed.",
            );
            writer.flush(&mut run, &[])?;
        }
        Ok(())
    }

    fn promotion_gate_in(
        &self,
        connection: &Connection,
        run: &PipelineRun,
        stage_id: &str,
    ) -> Result<(bool, String)> {
        let assessment = self.assess_in(
            connection,
            &PromotionContext {
                app_id: run.app_id.clone(),
                stage_id: stage_id.to_owned(),
                expected_manifest_hash: run.manifest.manifest_hash.clone(),
                expected_updated_at: run.registration_updated_at.clone(),
                release_ref: run.release_ref.clone(),
            },
            run.release_ref.clone(),
        )?;
        let ready = matches!(
            assessment.status,
            PromotionStatus::Ready | PromotionStatus::ReadyWithWarnings
        );
        let approved = ready
            && self
                .load_promotion_approvals(connection, Some(&run.app_id))?
                .iter()
                .any(|approval| approval.assessment.assessment_hash == assessment.assessment_hash);
        let message = if approved {
            "Current promotion criteria explicitly approved.".to_owned()
        } else if ready {
            "Promotion criteria ready; explicitly approve this candidate in the promotion tab or with approve-promotion.".to_owned()
        } else {
            format!(
                "Promotion criteria blocked: {}. Report evidence for this candidate in the promotion tab.",
                assessment
                    .criteria
                    .iter()
                    .filter(|criterion| criterion.status == crate::CriterionStatus::Blocked)
                    .map(|criterion| criterion.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        };
        Ok((approved, message))
    }
}

impl RunWriter {
    pub(crate) fn control(&self, run_id: &str) -> Result<RunControl> {
        self.registry.require_version(&self.connection)?;
        read_control(&self.registry, &self.connection, run_id)
    }

    pub(crate) fn promotion_gate(
        &mut self,
        run: &mut PipelineRun,
        stage: usize,
    ) -> Result<(bool, String)> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                self.registry
                    .storage("begin pipeline promotion gate", error)
            })?;
        self.registry.require_version(&transaction)?;
        self.registry.validate_schema(&transaction)?;
        if read_control(&self.registry, &transaction, &run.id)?.cancelled {
            return Ok((false, "Cancellation requested.".to_owned()));
        }
        let decision =
            self.registry
                .promotion_gate_in(&transaction, run, &run.stages[stage].stage_id)?;
        if decision.0 {
            // Evidence and stage advancement share this write transaction; a newer
            // failing report cannot slip between assessment and releasing descendants.
            run.stages[stage].status = RunStatus::Passed;
            run.stages[stage].finished_at = Some(now());
            run.stages[stage].message = None;
            let snapshot = serde_json::to_string(run)
                .map_err(|error| self.registry.storage("encode promoted stage", error))?;
            let updated = transaction
                .execute(
                    "UPDATE pipeline_runs SET snapshot = ?1 WHERE id = ?2 AND active = 1",
                    params![snapshot, run.id],
                )
                .map_err(|error| self.registry.storage("save promoted stage", error))?;
            if updated != 1 {
                return Err(self
                    .registry
                    .storage("save promoted stage", "run is no longer active"));
            }
            insert_logs(
                &self.registry,
                &transaction,
                &run.id,
                &[RunLogEntry {
                    sequence: 0,
                    recorded_at: now(),
                    stage_id: Some(run.stages[stage].stage_id.clone()),
                    command_id: None,
                    stream: LogStream::System,
                    text: "Current promotion criteria approval verified; stage passed.\n"
                        .to_owned(),
                }],
            )?;
        }
        transaction.commit().map_err(|error| {
            self.registry
                .storage("commit pipeline promotion gate", error)
        })?;
        Ok(decision)
    }

    pub(crate) fn flush(&mut self, run: &mut PipelineRun, entries: &[RunLogEntry]) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| self.registry.storage("begin run progress write", error))?;
        self.registry.require_version(&transaction)?;
        self.registry.validate_schema(&transaction)?;
        if !run.status.is_active()
            && run.status != RunStatus::Interrupted
            && read_control(&self.registry, &transaction, &run.id)?.cancelled
        {
            run.status = RunStatus::Cancelled;
            run.message = Some("Run cancelled by request.".to_owned());
        }
        let snapshot = serde_json::to_string(run)
            .map_err(|error| self.registry.storage("encode run progress", error))?;
        let updated = transaction
            .execute(
                "UPDATE pipeline_runs SET snapshot = ?1, active = ?2 WHERE id = ?3 AND active = 1",
                params![snapshot, run.status.is_active(), run.id],
            )
            .map_err(|error| self.registry.storage("save run progress", error))?;
        if updated != 1 {
            return Err(self.registry.storage(
                "save run progress",
                "run ownership was lost or the run already finished",
            ));
        }
        insert_logs(&self.registry, &transaction, &run.id, entries)?;
        if !run.status.is_active() {
            insert_logs(
                &self.registry,
                &transaction,
                &run.id,
                &[RunLogEntry {
                    sequence: 0,
                    recorded_at: now(),
                    stage_id: None,
                    command_id: None,
                    stream: LogStream::System,
                    text: format!(
                        "Run finished: {:?}. {}\n",
                        run.status,
                        run.message.as_deref().unwrap_or("")
                    ),
                }],
            )?;
        }
        transaction
            .commit()
            .map_err(|error| self.registry.storage("commit run progress", error))
    }
}

pub(crate) fn interrupt(run: &mut PipelineRun, message: &str) {
    let finished = now();
    run.status = RunStatus::Interrupted;
    run.finished_at = Some(finished.clone());
    run.message = Some(message.to_owned());
    for stage in &mut run.stages {
        if stage.status.is_active() {
            stage.status = RunStatus::Interrupted;
            stage.finished_at = Some(finished.clone());
            stage.message = Some(message.to_owned());
        }
    }
    for command in &mut run.commands {
        if command.status.is_active() {
            command.status = if command.status == RunStatus::Running {
                RunStatus::Interrupted
            } else {
                RunStatus::Skipped
            };
            command.finished_at = Some(finished.clone());
            command.message = Some(message.to_owned());
        }
    }
}

fn read_control(registry: &Registry, connection: &Connection, run_id: &str) -> Result<RunControl> {
    let (cancelled, approvals): (bool, String) = connection
        .query_row(
            "SELECT cancel_requested, stage_approvals FROM pipeline_runs WHERE id = ?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| registry.storage("read run control", error))?;
    let approvals: BTreeMap<String, String> = serde_json::from_str(&approvals)
        .map_err(|error| registry.storage("decode run approvals", error))?;
    for (stage, timestamp) in &approvals {
        validation::slug(stage)
            .map_err(|error| registry.storage("decode approval stage", error))?;
        validation::utc_timestamp(timestamp)
            .map_err(|error| registry.storage("decode approval time", error))?;
    }
    Ok(RunControl {
        cancelled,
        approvals,
    })
}

fn insert_logs(
    registry: &Registry,
    connection: &Connection,
    run_id: &str,
    entries: &[RunLogEntry],
) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut statement = connection.prepare(
        "INSERT INTO pipeline_run_logs (run_id, recorded_at, stage_id, command_id, stream, text)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
    ).map_err(|error| registry.storage("prepare pipeline output write", error))?;
    for entry in entries {
        let stream = match entry.stream {
            LogStream::Stdout => "stdout",
            LogStream::Stderr => "stderr",
            LogStream::System => "system",
        };
        statement
            .execute(params![
                run_id,
                entry.recorded_at,
                entry.stage_id,
                entry.command_id,
                stream,
                entry.text
            ])
            .map_err(|error| registry.storage("save pipeline output", error))?;
        if connection.last_insert_rowid() > MAX_LOG_CURSOR as i64 {
            return Err(registry.storage(
                "save pipeline output",
                "log sequence exceeds the exact JSON cursor range",
            ));
        }
    }
    Ok(())
}

struct StoredRun {
    id: i64,
    app_id: String,
    source_path: String,
    snapshot: String,
    active: bool,
}

impl StoredRun {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            app_id: row.get(1)?,
            source_path: row.get(2)?,
            snapshot: row.get(3)?,
            active: row.get(4)?,
        })
    }

    fn decode(self, registry: &Registry) -> Result<PipelineRun> {
        let invalid = |message| registry.storage("decode pipeline run", message);
        let run: PipelineRun =
            serde_json::from_str(&self.snapshot).map_err(|error| invalid(error.to_string()))?;
        if self.id <= 0
            || run.id != self.id.to_string()
            || run.app_id != self.app_id
            || run.app_id != run.manifest.app_id
            || run.app_name != run.manifest.name
            || run.manifest.source_path != self.source_path
            || run.status.is_active() != self.active
            || run.max_parallel == 0
        {
            return Err(invalid(
                "run identity, provenance or activity does not match its saved snapshot".to_owned(),
            ));
        }
        if !matches!(
            run.status,
            RunStatus::Pending
                | RunStatus::Running
                | RunStatus::WaitingApproval
                | RunStatus::Passed
                | RunStatus::Failed
                | RunStatus::Cancelled
                | RunStatus::Interrupted
        ) {
            return Err(invalid("invalid pipeline status".to_owned()));
        }
        validate_snapshot(&run.manifest, SnapshotVersion::RunsV6)
            .map_err(|error| invalid(error.to_string()))?;
        validation::utc_timestamp(&run.started_at).map_err(invalid)?;
        validation::utc_timestamp(&run.registration_updated_at).map_err(invalid)?;
        if let Some(time) = &run.finished_at {
            validation::utc_timestamp(time).map_err(invalid)?;
        }
        if run.status.is_active() == run.finished_at.is_some()
            || validation::normalize_ref(&run.release_ref).map_err(invalid)? != run.release_ref
        {
            return Err(invalid(
                "invalid run completion or release reference".to_owned(),
            ));
        }
        if run.stages.iter().map(|stage| &stage.stage_id).ne(run
            .manifest
            .stages
            .iter()
            .map(|stage| &stage.id))
        {
            return Err(invalid("run stages differ from the saved plan".to_owned()));
        }
        let expected = run
            .manifest
            .prepare
            .iter()
            .map(|command| (None, command.id.as_str()))
            .chain(run.manifest.stages.iter().flat_map(|stage| {
                stage
                    .checks
                    .iter()
                    .map(move |command| (Some(stage.id.as_str()), command.id.as_str()))
            }));
        if run
            .commands
            .iter()
            .map(|command| (command.stage_id.as_deref(), command.command_id.as_str()))
            .ne(expected)
        {
            return Err(invalid(
                "run commands differ from the saved plan".to_owned(),
            ));
        }
        for (stage, configured) in run.stages.iter().zip(&run.manifest.stages) {
            if matches!(stage.status, RunStatus::TimedOut | RunStatus::Skipped)
                || (stage.approved_at.is_some() && !configured.requires_approval)
                || (stage.status == RunStatus::Passed
                    && configured.requires_approval
                    && stage.approved_at.is_none())
            {
                return Err(invalid("invalid stage status or approval".to_owned()));
            }
            validate_times(
                stage.status,
                stage.started_at.as_deref(),
                stage.finished_at.as_deref(),
            )
            .map_err(invalid)?;
            if let Some(time) = &stage.approved_at {
                validation::utc_timestamp(time).map_err(invalid)?;
            }
        }
        for command in &run.commands {
            if matches!(
                command.status,
                RunStatus::WaitingApproval | RunStatus::Blocked
            ) || (command.status == RunStatus::Passed && command.exit_code.is_none())
            {
                return Err(invalid("invalid command status or exit code".to_owned()));
            }
            validate_times(
                command.status,
                command.started_at.as_deref(),
                command.finished_at.as_deref(),
            )
            .map_err(invalid)?;
        }
        if run.status == RunStatus::Passed
            && (run.stages.iter().any(|stage| !stage.status.is_success())
                || run
                    .commands
                    .iter()
                    .any(|command| !command.status.is_success()))
        {
            return Err(invalid(
                "a passing run contains an unfinished or failed prerequisite".to_owned(),
            ));
        }
        Ok(run)
    }
}

fn validate_id(run_id: &str) -> Result<()> {
    validation::insertion_id(run_id).map_err(invalid_run)
}

fn validate_times(
    status: RunStatus,
    started: Option<&str>,
    finished: Option<&str>,
) -> std::result::Result<(), String> {
    if status.is_active() == finished.is_some()
        || (matches!(
            status,
            RunStatus::Running | RunStatus::Passed | RunStatus::TimedOut
        ) && started.is_none())
    {
        return Err("invalid execution timestamps for status".to_owned());
    }
    for time in [started, finished].into_iter().flatten() {
        validation::utc_timestamp(time)?;
    }
    Ok(())
}

fn invalid_run(message: impl ToString) -> Error {
    Error::InvalidRun {
        message: message.to_string(),
    }
}

fn conflict(message: &str) -> Error {
    Error::RunConflict {
        message: message.to_owned(),
    }
}
