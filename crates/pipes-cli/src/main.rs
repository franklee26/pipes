use std::{
    env,
    ffi::OsString,
    io::{self, Write},
    num::NonZeroU32,
    path::PathBuf,
    process::ExitCode,
};

use clap::{Args, Parser, Subcommand, error::ErrorKind};
use pipes_core::{
    ApprovePromotion, CheckPlanNode, CommandResult, CommandSummary, ConsumerApproval,
    ConsumerDeployment, CriterionStatus, DeploymentRequirement, HarnessResult, ManifestPreview,
    Parallelism, PipelineRun, PipelineRunSummary, PromotionApproval, PromotionAssessment,
    PromotionContext, PromotionCriterion, PromotionStatus, RecordConsumerDeployment,
    RecordHarnessResult, RegisteredApp, Registry, RunLogPage, StartPipelineRun, default_data_dir,
    preview_manifest,
};
use serde::Serialize;

mod runner;

#[derive(Parser)]
#[command(
    name = "pipes",
    version,
    about = "Review release plans, run approved pipelines, and inspect durable run history",
    long_about = "Preview release plans and register reviewed snapshots without executing commands.\n\
                  Use run --approve to execute a registered pipeline in the attached local process; stage gates require separate approval.\n\
                  Runs require source.snapshot: working-tree; commands use argv, not an implicit shell. Git-commit snapshots are not implemented.\n\
                  approval.local gates commands before execution via approve-stage; configured promotion.criteria gates completion after commands pass via approve-promotion.\n\
                  Promotion approval must currently match the run's release ref and registration. JSON-file verdicts never supply harness counts automatically.\n\
                  Report external harness results, record manual deployments, and explicitly approve declared promotion criteria.\n\
                  Evidence recording never deploys files or independently verifies external harness execution.\n\
                  Unregistering an app retains pipeline runs, logs, deployment, harness-result, and promotion-approval history."
)]
struct Cli {
    /// Local registry directory (overrides PIPES_DATA_DIR and the platform default).
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<PathBuf>,

    /// Emit JSON results and structured JSON errors.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Validate and preview YAML without opening or creating a registry.
    Validate { manifest: PathBuf },
    /// Register a validated snapshot. No commands are executed.
    Register {
        manifest: PathBuf,
        /// Explicitly replace changed contents or the path of an existing app ID.
        #[arg(long)]
        replace: bool,
    },
    /// List saved registrations, sorted by app ID.
    List,
    /// Inspect a saved snapshot, even if its original YAML no longer exists.
    Inspect { app_id: String },
    /// Execute an approved registered pipeline; stream output and wait for its final status.
    #[command(
        long_about = "Execute a registered pipeline with explicit consent, stream incremental output to stderr, and wait for completion.\n\
        Review the saved snapshot with inspect before approving execution. Use paired review tokens to reject stale registrations.\n\
        Commands run in the opted-in working tree (source.snapshot: working-tree); no implicit shell is used.\n\
        source.require_clean is checked in cancellable Git preflight; a failure is recorded before any manifest command executes.\n\
        This process owns the run: Ctrl-C requests cancellation and waits for child cleanup. Normal owner shutdown interrupts active work.\n\
        approval.local gates stage commands BEFORE execution; use approve-stage from another process while this command waits.\n\
        Configured promotion.criteria gates stage completion AFTER commands pass until a current approve-promotion decision matches the run's release ref and registration.\n\
        Neither gate is auto-approved. JSON-file command results select pass/fail only; report-harness still records external counts explicitly.\n\
        With --json, stdout is one final PipelineRun object; stderr is NDJSON output events. Non-passing final runs exit nonzero."
    )]
    Run {
        app_id: String,
        /// Release ref identifying this run.
        #[arg(long, value_name = "REF")]
        release_ref: String,
        /// Explicitly approve executing this registered snapshot.
        #[arg(long, required = true)]
        approve: bool,
        #[command(flatten)]
        review: ReviewToken,
    },
    /// List the newest 100 pipeline runs, including unregistered apps.
    Runs { app_id: Option<String> },
    /// Inspect the durable state of a pipeline run.
    RunStatus { run_id: String },
    /// Read one bounded page of output; use nextCursor when hasMore is true.
    Logs {
        run_id: String,
        /// Return entries strictly after this cursor (default: from the beginning).
        #[arg(long, default_value_t = 0, value_name = "CURSOR")]
        after: u64,
    },
    /// Request cancellation of an active run owned by a CLI or desktop process.
    CancelRun { run_id: String },
    /// Approve an approval.local gate before stage commands run, not a promotion completion gate.
    ApproveStage {
        run_id: String,
        #[arg(long)]
        stage: String,
        /// Approve this waiting stage. Starting a run never grants this approval.
        #[arg(long, required = true)]
        approve: bool,
    },
    /// Record a manually approved deployment to a manifest-defined consumer. Nothing is executed.
    DeployedTo {
        app_id: String,
        /// Source stage containing the consumer.
        #[arg(long)]
        stage: String,
        /// Destination consumer ID from the source stage.
        #[arg(long)]
        consumer: String,
        /// Release ref that was deployed outside Pipes.
        #[arg(long, value_name = "REF")]
        release_ref: String,
        /// Explicitly approve recording this deployment.
        #[arg(long, required = true)]
        approve: bool,
        /// Optional deployment note.
        #[arg(long, value_name = "TEXT")]
        note: Option<String>,
        #[command(flatten)]
        review: ReviewToken,
    },
    /// List immutable manual deployment history, including unregistered apps.
    Deployments { app_id: Option<String> },
    /// Record external harness counts. No approval is needed; no harness is run or verified.
    ReportHarness {
        #[command(flatten)]
        candidate: PromotionCandidate,
        /// Command-leaf check ID from the source stage.
        #[arg(long)]
        check: String,
        /// Total tests reported by the external harness.
        #[arg(long, value_name = "N")]
        total: u32,
        /// Passing tests reported by the external harness (must not exceed total).
        #[arg(long, value_name = "M")]
        passed: u32,
        /// Optional external-result note.
        #[arg(long, value_name = "TEXT")]
        note: Option<String>,
    },
    /// List immutable external harness-result history, including unregistered apps.
    HarnessResults { app_id: Option<String> },
    /// Assess declared criteria. Blocked/not-configured assessments still exit 0; inspect status.
    #[command(
        long_about = "Assess only the source stage's declared promotion criteria using reported evidence for this candidate and registration.\n\
        This assessment evaluates promotion.criteria only; the runner enforces approval.local separately before stage commands.\n\
        A successful assessment exits 0 even when status is blocked or not_configured: callers must inspect status.\n\
        Use approve-promotion as the enforcing gate. Assessment does not execute commands or advance stages."
    )]
    PromotionStatus {
        #[command(flatten)]
        candidate: PromotionCandidate,
    },
    /// Record an explicitly approved, ready promotion decision for a runner policy gate.
    #[command(
        long_about = "Record a promotion decision only; this command does not execute stage commands.\n\
        After commands pass, an attached runner observes currently matching decisions for its release ref and registration before completing the stage.\n\
        This does not approve an approval.local pre-command gate. No approval or external harness counts are generated automatically."
    )]
    ApprovePromotion {
        #[command(flatten)]
        candidate: PromotionCandidate,
        /// Explicitly approve recording this promotion-criteria decision.
        #[arg(long, required = true)]
        approve: bool,
        /// Explicitly acknowledge and record every skipped optional criterion.
        #[arg(long)]
        acknowledge_optional: bool,
        /// Optional approval note.
        #[arg(long, value_name = "TEXT")]
        note: Option<String>,
        /// Previously reviewed assessment fingerprint; stale values are rejected, not refreshed.
        #[arg(long, value_name = "HASH")]
        expected_assessment_hash: Option<String>,
    },
    /// List immutable promotion approvals and skipped optional steps, including unregistered apps.
    PromotionApprovals { app_id: Option<String> },
    /// Remove a registration without cancelling active runs; retain history and source/manifest files.
    #[command(
        long_about = "Remove a registration while retaining run, output, evidence, and approval history and all source/manifest files.\n\
        Active runs continue using their frozen snapshots. Use cancel-run and wait for cleanup before unregistering if execution should stop."
    )]
    Unregister { app_id: String },
}

#[derive(Args)]
struct ReviewToken {
    /// Previously reviewed manifest hash; requires --expected-updated-at.
    #[arg(long, value_name = "HASH", requires = "expected_updated_at")]
    expected_hash: Option<String>,
    /// Previously reviewed registration timestamp; requires --expected-hash.
    #[arg(long, value_name = "TIMESTAMP", requires = "expected_hash")]
    expected_updated_at: Option<String>,
}

impl ReviewToken {
    fn resolve(&self, registry: &Registry, app_id: &str) -> pipes_core::Result<(String, String)> {
        if let Some((hash, updated_at)) = self
            .expected_hash
            .as_ref()
            .zip(self.expected_updated_at.as_ref())
        {
            return Ok((hash.clone(), updated_at.clone()));
        }
        let registration = registry.get(app_id)?;
        Ok((registration.manifest.manifest_hash, registration.updated_at))
    }
}

#[derive(Args)]
struct PromotionCandidate {
    app_id: String,
    /// Source stage declaring the promotion policy.
    #[arg(long)]
    stage: String,
    /// Candidate release ref to assess or record evidence for.
    #[arg(long, value_name = "REF")]
    release_ref: String,
    #[command(flatten)]
    review: ReviewToken,
}

impl PromotionCandidate {
    fn context(&self, registry: &Registry) -> pipes_core::Result<PromotionContext> {
        let (expected_manifest_hash, expected_updated_at) =
            self.review.resolve(registry, &self.app_id)?;
        Ok(PromotionContext {
            app_id: self.app_id.clone(),
            stage_id: self.stage.clone(),
            expected_manifest_hash,
            expected_updated_at,
            release_ref: self.release_ref.clone(),
        })
    }
}

enum Output {
    Preview(ManifestPreview),
    Registered(RegisteredApp),
    Inspected(RegisteredApp),
    Listed(Vec<RegisteredApp>),
    RunCompleted(PipelineRun),
    RunInspected(PipelineRun),
    RunsListed(Vec<PipelineRunSummary>),
    LogsListed(RunLogPage),
    DeploymentRecorded(ConsumerDeployment),
    DeploymentsListed(Vec<ConsumerDeployment>),
    HarnessResultRecorded(HarnessResult),
    HarnessResultsListed(Vec<HarnessResult>),
    PromotionAssessed(PromotionAssessment),
    PromotionApproved(PromotionApproval),
    PromotionApprovalsListed(Vec<PromotionApproval>),
    Unregistered(Unregistered),
}

enum CliError {
    Core(pipes_core::Error),
    Output(io::Error),
}

impl CliError {
    fn code(&self) -> &'static str {
        match self {
            Self::Core(error) => error.code(),
            Self::Output(_) => "output",
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core(error) => error.fmt(formatter),
            Self::Output(error) => error.fmt(formatter),
        }
    }
}

impl From<pipes_core::Error> for CliError {
    fn from(error: pipes_core::Error) -> Self {
        Self::Core(error)
    }
}

impl From<io::Error> for CliError {
    fn from(error: io::Error) -> Self {
        Self::Output(error)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Unregistered {
    app_id: String,
    unregistered: bool,
}

#[derive(Serialize)]
struct ErrorResponse<'a> {
    error: ErrorDetail<'a>,
}

#[derive(Serialize)]
struct ErrorDetail<'a> {
    code: &'a str,
    message: &'a str,
}

fn main() -> ExitCode {
    let arguments: Vec<OsString> = env::args_os().collect();
    let json_requested = arguments
        .iter()
        .skip(1)
        .take_while(|argument| *argument != "--")
        .any(|argument| argument == "--json");
    let cli = match Cli::try_parse_from(arguments) {
        Ok(cli) => cli,
        Err(error) => {
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                return match error.print() {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(error) => {
                        emit_error(json_requested, "output", &error.to_string());
                        ExitCode::FAILURE
                    }
                };
            }
            emit_error(json_requested, "usage", &error.to_string());
            return ExitCode::from(2);
        }
    };
    match run(&cli) {
        Ok(output) => {
            let success = !matches!(&output, Output::RunCompleted(run) if !run.status.is_success());
            match write_output(output, cli.json, &mut io::stdout().lock()) {
                Ok(()) if success => ExitCode::SUCCESS,
                Ok(()) => ExitCode::FAILURE,
                Err(error) => {
                    emit_error(cli.json, "output", &error.to_string());
                    ExitCode::FAILURE
                }
            }
        }
        Err(error) => {
            emit_error(cli.json, error.code(), &error.to_string());
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<Output, CliError> {
    if let Commands::Validate { manifest } = &cli.command {
        return Ok(Output::Preview(preview_manifest(manifest)?));
    }
    let data_dir = match &cli.data_dir {
        Some(path) => path.clone(),
        None => default_data_dir()?,
    };
    let registry = Registry::open(data_dir)?;
    let output = match &cli.command {
        Commands::Validate { manifest } => preview_manifest(manifest).map(Output::Preview),
        Commands::Register { manifest, replace } => registry
            .register(manifest, None, *replace)
            .map(Output::Registered),
        Commands::List => registry.list().map(Output::Listed),
        Commands::Inspect { app_id } => registry.get(app_id).map(Output::Inspected),
        Commands::Run {
            app_id,
            release_ref,
            approve,
            review,
        } => {
            let (expected_manifest_hash, expected_updated_at) =
                review.resolve(&registry, app_id)?;
            return runner::execute(
                &registry,
                &StartPipelineRun {
                    app_id: app_id.clone(),
                    expected_manifest_hash,
                    expected_updated_at,
                    release_ref: release_ref.clone(),
                    approved: *approve,
                },
                cli.json,
            )
            .map(Output::RunCompleted);
        }
        Commands::Runs { app_id } => registry
            .pipeline_runs(app_id.as_deref())
            .map(Output::RunsListed),
        Commands::RunStatus { run_id } => registry.pipeline_run(run_id).map(Output::RunInspected),
        Commands::Logs { run_id, after } => registry
            .pipeline_run_logs(run_id, *after)
            .map(Output::LogsListed),
        Commands::CancelRun { run_id } => registry
            .cancel_pipeline_run(run_id)
            .map(Output::RunInspected),
        Commands::ApproveStage {
            run_id,
            stage,
            approve,
        } => registry
            .approve_pipeline_stage(run_id, stage, *approve)
            .map(Output::RunInspected),
        Commands::DeployedTo {
            app_id,
            stage,
            consumer,
            release_ref,
            approve,
            note,
            review,
        } => {
            let (expected_manifest_hash, expected_updated_at) =
                review.resolve(&registry, app_id)?;
            registry
                .record_consumer_deployment(&RecordConsumerDeployment {
                    app_id: app_id.clone(),
                    stage_id: stage.clone(),
                    consumer_id: consumer.clone(),
                    expected_manifest_hash,
                    expected_updated_at,
                    release_ref: release_ref.clone(),
                    notes: note.clone(),
                    approved: *approve,
                })
                .map(Output::DeploymentRecorded)
        }
        Commands::Deployments { app_id } => registry
            .consumer_deployments(app_id.as_deref())
            .map(Output::DeploymentsListed),
        Commands::ReportHarness {
            candidate,
            check,
            total,
            passed,
            note,
        } => {
            let context = candidate.context(&registry)?;
            registry
                .record_harness_result(&RecordHarnessResult {
                    app_id: context.app_id,
                    stage_id: context.stage_id,
                    expected_manifest_hash: context.expected_manifest_hash,
                    expected_updated_at: context.expected_updated_at,
                    release_ref: context.release_ref,
                    check_id: check.clone(),
                    total: *total,
                    passed: *passed,
                    notes: note.clone(),
                })
                .map(Output::HarnessResultRecorded)
        }
        Commands::HarnessResults { app_id } => registry
            .harness_results(app_id.as_deref())
            .map(Output::HarnessResultsListed),
        Commands::PromotionStatus { candidate } => registry
            .assess_promotion(&candidate.context(&registry)?)
            .map(Output::PromotionAssessed),
        Commands::ApprovePromotion {
            candidate,
            approve,
            acknowledge_optional,
            note,
            expected_assessment_hash,
        } => {
            let context = candidate.context(&registry)?;
            let expected_assessment_hash = match expected_assessment_hash {
                Some(hash) => hash.clone(),
                None => registry.assess_promotion(&context)?.assessment_hash,
            };
            registry
                .approve_promotion(&ApprovePromotion {
                    app_id: context.app_id,
                    stage_id: context.stage_id,
                    expected_manifest_hash: context.expected_manifest_hash,
                    expected_updated_at: context.expected_updated_at,
                    release_ref: context.release_ref,
                    expected_assessment_hash,
                    approved: *approve,
                    acknowledge_optional: *acknowledge_optional,
                    notes: note.clone(),
                })
                .map(Output::PromotionApproved)
        }
        Commands::PromotionApprovals { app_id } => registry
            .promotion_approvals(app_id.as_deref())
            .map(Output::PromotionApprovalsListed),
        Commands::Unregister { app_id } => {
            registry.unregister(app_id)?;
            Ok(Output::Unregistered(Unregistered {
                app_id: app_id.clone(),
                unregistered: true,
            }))
        }
    };
    output.map_err(CliError::from)
}

fn write_output(output: Output, json: bool, writer: &mut impl Write) -> io::Result<()> {
    if json {
        match &output {
            Output::Preview(value) => serde_json::to_writer(&mut *writer, value)?,
            Output::Registered(value) | Output::Inspected(value) => {
                serde_json::to_writer(&mut *writer, value)?;
            }
            Output::Listed(value) => serde_json::to_writer(&mut *writer, value)?,
            Output::RunCompleted(value) | Output::RunInspected(value) => {
                serde_json::to_writer(&mut *writer, value)?;
            }
            Output::RunsListed(value) => serde_json::to_writer(&mut *writer, value)?,
            Output::LogsListed(value) => serde_json::to_writer(&mut *writer, value)?,
            Output::DeploymentRecorded(value) => serde_json::to_writer(&mut *writer, value)?,
            Output::DeploymentsListed(value) => serde_json::to_writer(&mut *writer, value)?,
            Output::HarnessResultRecorded(value) => serde_json::to_writer(&mut *writer, value)?,
            Output::HarnessResultsListed(value) => serde_json::to_writer(&mut *writer, value)?,
            Output::PromotionAssessed(value) => serde_json::to_writer(&mut *writer, value)?,
            Output::PromotionApproved(value) => serde_json::to_writer(&mut *writer, value)?,
            Output::PromotionApprovalsListed(value) => serde_json::to_writer(&mut *writer, value)?,
            Output::Unregistered(value) => serde_json::to_writer(&mut *writer, value)?,
        }
        return writeln!(writer);
    }
    match output {
        Output::Preview(manifest) => {
            writeln!(writer, "Valid: {} ({})", manifest.name, manifest.app_id)?;
            write_manifest(&manifest, writer)?;
            writeln!(writer, "No commands were executed.")
        }
        Output::Registered(app) => {
            writeln!(
                writer,
                "Registered: {} ({})",
                app.manifest.name, app.manifest.app_id
            )?;
            write_manifest(&app.manifest, writer)?;
            writeln!(writer, "No commands were executed.")
        }
        Output::Inspected(app) => {
            writeln!(writer, "{} ({})", app.manifest.name, app.manifest.app_id)?;
            write_manifest(&app.manifest, writer)?;
            writeln!(writer, "Registered: {}", app.registered_at)?;
            writeln!(writer, "Updated: {}", app.updated_at)
        }
        Output::Listed(apps) => {
            if apps.is_empty() {
                writeln!(writer, "No apps registered.")?;
            }
            for app in apps {
                writeln!(
                    writer,
                    "{}\t{}\t{}",
                    app.manifest.app_id, app.manifest.name, app.manifest.manifest_path
                )?;
            }
            Ok(())
        }
        Output::RunCompleted(run) | Output::RunInspected(run) => runner::write_run(&run, writer),
        Output::RunsListed(runs) => runner::write_runs(&runs, writer),
        Output::LogsListed(page) => {
            for entry in &page.entries {
                runner::write_log(entry, writer)?;
            }
            writeln!(
                writer,
                "Next cursor: {} (has more: {})",
                page.next_cursor, page.has_more
            )
        }
        Output::DeploymentRecorded(deployment) => {
            writeln!(writer, "Recorded manual deployment:")?;
            write_deployment(&deployment, writer)?;
            writeln!(
                writer,
                "No files were copied or deployed. No commands were executed."
            )
        }
        Output::DeploymentsListed(deployments) => {
            if deployments.is_empty() {
                return writeln!(writer, "No manual deployments recorded.");
            }
            writeln!(
                writer,
                "Manual deployment history (newest first; retained after unregistration):"
            )?;
            for deployment in deployments {
                write_deployment(&deployment, writer)?;
            }
            Ok(())
        }
        Output::HarnessResultRecorded(result) => {
            writeln!(writer, "Recorded external harness result:")?;
            write_harness_result(&result, writer)?;
            writeln!(
                writer,
                "No harness was executed or independently verified. An attached runner still requires explicit promotion approval."
            )
        }
        Output::HarnessResultsListed(results) => {
            if results.is_empty() {
                return writeln!(writer, "No external harness results recorded.");
            }
            writeln!(
                writer,
                "External harness-result history (newest first; retained after unregistration):"
            )?;
            for result in results {
                write_harness_result(&result, writer)?;
            }
            Ok(())
        }
        Output::PromotionAssessed(assessment) => {
            write_assessment(&assessment, writer)?;
            writeln!(
                writer,
                "Assessment only; inspect status. Use approve-promotion to record an explicit policy decision. This command did not execute commands or approve stage completion."
            )
        }
        Output::PromotionApproved(approval) => {
            writeln!(writer, "Promotion-criteria approval recorded:")?;
            write_promotion_approval(&approval, writer)?;
            writeln!(
                writer,
                "This command did not execute commands or deploy files. After commands pass, an attached runner can release its completion gate when this approval currently matches its release ref and registration."
            )
        }
        Output::PromotionApprovalsListed(approvals) => {
            if approvals.is_empty() {
                return writeln!(writer, "No promotion-criteria approvals recorded.");
            }
            writeln!(
                writer,
                "Promotion-criteria approval history (newest first; retained after unregistration):"
            )?;
            for approval in approvals {
                write_promotion_approval(&approval, writer)?;
            }
            Ok(())
        }
        Output::Unregistered(confirmation) => writeln!(
            writer,
            "Unregistered '{}'. Deployment history was retained. Harness-result and promotion-approval history were retained. Pipeline-run history and logs were retained. Source and manifest files were not changed. Active runs continue using their frozen snapshots; use cancel-run if execution should stop.",
            confirmation.app_id
        ),
    }
}

fn approval_description(approval: &ConsumerApproval) -> &'static str {
    match approval {
        ConsumerApproval::Manual => "manual",
    }
}

fn write_deployment(deployment: &ConsumerDeployment, writer: &mut impl Write) -> io::Result<()> {
    writeln!(writer, "  ID: {}", deployment.id)?;
    writeln!(writer, "  Recorded: {}", deployment.recorded_at)?;
    writeln!(
        writer,
        "  App: {} ({})",
        deployment.app_name, deployment.app_id
    )?;
    writeln!(writer, "  Source stage: {}", deployment.stage_id)?;
    writeln!(
        writer,
        "  Consumer: {} ({})",
        deployment.consumer.name, deployment.consumer.id
    )?;
    writeln!(writer, "  Target: {}", deployment.consumer.target)?;
    writeln!(writer, "  Ref: {}", deployment.release_ref)?;
    writeln!(
        writer,
        "  Approval: {}",
        approval_description(&deployment.consumer.approval)
    )?;
    if let Some(notes) = &deployment.notes {
        writeln!(writer, "  Note: {notes}")?;
    }
    Ok(())
}

fn write_harness_result(result: &HarnessResult, writer: &mut impl Write) -> io::Result<()> {
    writeln!(writer, "  ID: {}", result.id)?;
    writeln!(writer, "  Recorded: {}", result.recorded_at)?;
    writeln!(writer, "  App: {} ({})", result.app_name, result.app_id)?;
    writeln!(writer, "  Source stage: {}", result.stage_id)?;
    writeln!(writer, "  Check: {}", result.check_id)?;
    writeln!(writer, "  Ref: {}", result.release_ref)?;
    writeln!(
        writer,
        "  Reported: {}/{} passed",
        result.passed, result.total
    )?;
    writeln!(writer, "  Manifest hash: {}", result.manifest_hash)?;
    writeln!(
        writer,
        "  Registration updated: {}",
        result.registration_updated_at
    )?;
    if let Some(notes) = &result.notes {
        writeln!(writer, "  Note: {notes}")?;
    }
    Ok(())
}

fn write_criterion(criterion: &PromotionCriterion, writer: &mut impl Write) -> io::Result<()> {
    match criterion {
        PromotionCriterion::Harness {
            id,
            check_id,
            expected_tests,
            minimum_passed,
        } => writeln!(
            writer,
            "  {id}: harness {check_id}; exactly {expected_tests} tests, minimum {minimum_passed} passed"
        ),
        PromotionCriterion::Deployment {
            id,
            consumer_id,
            requirement,
        } => {
            let requirement = match requirement {
                DeploymentRequirement::Required => "required",
                DeploymentRequirement::Optional => "optional",
            };
            writeln!(
                writer,
                "  {id}: {requirement} manual deployment to {consumer_id}"
            )
        }
    }
}

fn write_assessment(assessment: &PromotionAssessment, writer: &mut impl Write) -> io::Result<()> {
    let status = match assessment.status {
        PromotionStatus::NotConfigured => "not_configured",
        PromotionStatus::Blocked => "blocked",
        PromotionStatus::Ready => "ready",
        PromotionStatus::ReadyWithWarnings => "ready_with_warnings",
    };
    writeln!(writer, "Promotion status: {status}")?;
    writeln!(
        writer,
        "  App: {} ({})",
        assessment.app_name, assessment.app_id
    )?;
    writeln!(writer, "  Source stage: {}", assessment.stage_id)?;
    writeln!(writer, "  Ref: {}", assessment.release_ref)?;
    writeln!(writer, "  Manifest: {}", assessment.manifest_path)?;
    writeln!(writer, "  Manifest hash: {}", assessment.manifest_hash)?;
    writeln!(writer, "  Source: {}", assessment.source_path)?;
    writeln!(
        writer,
        "  Registration updated: {}",
        assessment.registration_updated_at
    )?;
    writeln!(writer, "  Assessment hash: {}", assessment.assessment_hash)?;
    if assessment.criteria.is_empty() {
        writeln!(
            writer,
            "  No promotion criteria configured; approval is unavailable."
        )?;
    }
    for evaluation in &assessment.criteria {
        write_criterion(&evaluation.criterion, writer)?;
        let status = match evaluation.status {
            CriterionStatus::Satisfied => "satisfied",
            CriterionStatus::Blocked => "blocked",
            CriterionStatus::Warning => "warning",
        };
        writeln!(writer, "    {status}: {}", evaluation.message)?;
        if let Some(observed) = &evaluation.observed {
            writeln!(
                writer,
                "    Observed: {}/{} passed",
                observed.passed, observed.total
            )?;
        }
        if let Some(consumer) = &evaluation.consumer {
            writeln!(
                writer,
                "    Consumer: {} ({}) -> {} ({} approval)",
                consumer.name,
                consumer.id,
                consumer.target,
                approval_description(&consumer.approval)
            )?;
        }
        if let Some(evidence_id) = &evaluation.evidence_id {
            writeln!(writer, "    Evidence ID: {evidence_id}")?;
        }
    }
    if !assessment.stage_requirements.is_empty() {
        writeln!(
            writer,
            "  Stage requirements (not evaluated in this promotion assessment): {}",
            assessment.stage_requirements.join(", ")
        )?;
    }
    Ok(())
}

fn write_promotion_approval(
    approval: &PromotionApproval,
    writer: &mut impl Write,
) -> io::Result<()> {
    writeln!(writer, "  Approval ID: {}", approval.id)?;
    writeln!(writer, "  Recorded: {}", approval.recorded_at)?;
    write_assessment(&approval.assessment, writer)?;
    if approval.skipped_optional.is_empty() {
        writeln!(writer, "  Skipped optional criteria: none")?;
    } else {
        writeln!(
            writer,
            "  Skipped optional criteria (acknowledged): {}",
            approval.skipped_optional.join(", ")
        )?;
    }
    if let Some(notes) = &approval.notes {
        writeln!(writer, "  Note: {notes}")?;
    }
    Ok(())
}

fn write_manifest(manifest: &ManifestPreview, writer: &mut impl Write) -> io::Result<()> {
    writeln!(writer, "Manifest: {}", manifest.manifest_path)?;
    writeln!(writer, "Source: {}", manifest.source_path)?;
    writeln!(writer, "Version from: {}", manifest.version_from)?;
    let limits = PlanLimits::from(manifest.execution.max_parallel);
    writeln!(
        writer,
        "Command budget: {} (shared run-wide)",
        limits.description()
    )?;
    if !manifest.prepare.is_empty() {
        writeln!(writer, "Prepare results:")?;
        write_command_results(&manifest.prepare, writer)?;
    }
    let linear = manifest.stages.iter().enumerate().all(|(index, stage)| {
        let previous = index
            .checked_sub(1)
            .and_then(|previous| manifest.stages.get(previous))
            .map(|previous| previous.id.as_str());
        stage.after.iter().map(String::as_str).eq(previous)
    });
    if linear {
        writeln!(
            writer,
            "Stages: {}",
            manifest
                .stages
                .iter()
                .map(|stage| stage.id.as_str())
                .collect::<Vec<_>>()
                .join(" -> ")
        )?;
    } else {
        writeln!(writer, "Stages (all incoming parents required):")?;
        for stage in &manifest.stages {
            if stage.after.is_empty() {
                writeln!(writer, "  {}: root", stage.id)?;
            } else {
                writeln!(writer, "  {}: after {}", stage.id, stage.after.join(", "))?;
            }
        }
    }
    for stage in &manifest.stages {
        if stage.requires_approval {
            writeln!(
                writer,
                "Execution gate {}: approval.local before commands; use approve-stage --approve.",
                stage.id
            )?;
        }
        if !stage.consumers.is_empty() {
            writeln!(writer, "Consumers {}:", stage.id)?;
            for consumer in &stage.consumers {
                writeln!(
                    writer,
                    "  {} ({}) -> {} ({} approval)",
                    consumer.name,
                    consumer.id,
                    consumer.target,
                    approval_description(&consumer.approval)
                )?;
            }
        }
        if stage.promotion_criteria.is_empty() {
            writeln!(writer, "Promotion criteria {}: not configured.", stage.id)?;
        } else {
            writeln!(writer, "Promotion criteria {}:", stage.id)?;
            for criterion in &stage.promotion_criteria {
                write_criterion(criterion, writer)?;
            }
            writeln!(
                writer,
                "Completion gate {}: after commands pass; requires a current approve-promotion decision for the run's release ref and registration.",
                stage.id
            )?;
        }
        if !stage.requires.is_empty() {
            writeln!(
                writer,
                "Stage requirements {} (not evaluated in this promotion assessment): {}",
                stage.id,
                stage.requires.join(", ")
            )?;
        }
        if stage.check_plan.is_empty() {
            continue;
        }
        if stage
            .check_plan
            .iter()
            .all(|node| matches!(node, CheckPlanNode::Command { .. }))
        {
            let ids: Vec<_> = stage
                .check_plan
                .iter()
                .filter_map(|node| match node {
                    CheckPlanNode::Command { command } => Some(command.id.as_str()),
                    _ => None,
                })
                .collect();
            writeln!(
                writer,
                "Checks {} (sequence): {}",
                stage.id,
                ids.join(" -> ")
            )?;
        } else {
            writeln!(
                writer,
                "Checks {} (sequence; advance only after each child passes):",
                stage.id
            )?;
            write_plan(&stage.check_plan, writer, 2, limits)?;
        }
        writeln!(writer, "Command results {}:", stage.id)?;
        write_command_results(&stage.checks, writer)?;
    }
    Ok(())
}

fn write_command_results(commands: &[CommandSummary], writer: &mut impl Write) -> io::Result<()> {
    for command in commands {
        match command.result.as_ref() {
            CommandResult::ExitCode { success_codes } => {
                let codes = success_codes
                    .iter()
                    .map(i32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(
                    writer,
                    "  {}: exit-code; success codes: {codes}",
                    command.id
                )?;
            }
            CommandResult::JsonFile {
                path,
                pointer,
                success,
                failure,
            } => writeln!(
                writer,
                "  {}: json-file {path}; pointer {pointer:?}; success: {success}; failure: {failure}",
                command.id
            )?,
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct PlanLimits {
    finite: Option<NonZeroU32>,
    auto: bool,
}

impl From<Parallelism> for PlanLimits {
    fn from(parallelism: Parallelism) -> Self {
        match parallelism {
            Parallelism::Auto => Self {
                finite: None,
                auto: true,
            },
            Parallelism::Limit(limit) => Self {
                finite: Some(limit),
                auto: false,
            },
        }
    }
}

impl PlanLimits {
    fn capped(self, cap: Option<NonZeroU32>) -> Self {
        let finite = match (self.finite, cap) {
            (Some(parent), Some(child)) => Some(parent.min(child)),
            (parent, None) => parent,
            (None, child) => child,
        };
        Self { finite, ..self }
    }

    fn description(self) -> String {
        match (self.finite, self.auto) {
            (Some(limit), true) => format!("<= {limit} commands and auto host capacity"),
            (Some(limit), false) => format!("<= {limit} commands"),
            (None, _) => "auto (host capacity unresolved)".to_owned(),
        }
    }
}

fn write_plan(
    nodes: &[CheckPlanNode],
    writer: &mut impl Write,
    indent: usize,
    limits: PlanLimits,
) -> io::Result<()> {
    for (index, node) in nodes.iter().enumerate() {
        write!(writer, "{:indent$}{}. ", "", index + 1)?;
        match node {
            CheckPlanNode::Command { command } => writeln!(writer, "command {}", command.id)?,
            CheckPlanNode::Sequence { id, steps } => {
                writeln!(writer, "sequence {id} (all required):")?;
                write_plan(steps, writer, indent + 2, limits)?;
            }
            CheckPlanNode::Parallel {
                id,
                steps,
                max_parallel,
            } => {
                let effective = limits.capped(*max_parallel);
                let cap = max_parallel.map_or_else(|| "inherit".to_owned(), |cap| cap.to_string());
                writeln!(
                    writer,
                    "parallel {id} (all required; cap {cap}; effective {}):",
                    effective.description()
                )?;
                write_plan(steps, writer, indent + 2, effective)?;
            }
        }
    }
    Ok(())
}

fn emit_error(json: bool, code: &str, message: &str) {
    let mut stderr = io::stderr().lock();
    if json {
        let response = ErrorResponse {
            error: ErrorDetail { code, message },
        };
        let _ = serde_json::to_writer(&mut stderr, &response);
        let _ = writeln!(stderr);
    } else {
        let _ = writeln!(stderr, "error [{code}]: {message}");
    }
}
