use std::{
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use pipes_core::{
    LogStream, PipelineRun, PipelineRunSummary, PipelineRunner, Registry, RunLogEntry, RunStatus,
    StartPipelineRun,
};
use serde::Serialize;

use crate::CliError;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OutputEvent<'a> {
    event: &'static str,
    run_id: &'a str,
    entry: &'a RunLogEntry,
}

pub(crate) fn execute(
    registry: &Registry,
    request: &StartPipelineRun,
    json: bool,
) -> Result<PipelineRun, CliError> {
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&interrupted);
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed)).map_err(|error| {
        pipes_core::Error::Execution {
            message: format!("Cannot install the Ctrl-C cancellation handler: {error}"),
        }
    })?;

    let runner = PipelineRunner::new(registry.clone());
    let run = runner.start(request)?;
    let mut stderr = io::stderr().lock();
    if !json {
        writeln!(
            stderr,
            "Run {}: {} ({}) at {}. Ctrl-C cancels. Local command gates use approve-stage; promotion completion gates use approve-promotion.",
            run.id, run.app_name, run.app_id, run.release_ref
        )?;
        stderr.flush()?;
    }
    let result = wait(registry, &run.id, &interrupted, json, &mut stderr);
    // Always join the owner, including when reading or streaming output fails.
    let shutdown = runner.shutdown();
    let mut cursor = result?;
    shutdown?;
    while stream_page(registry, &run.id, &mut cursor, json, &mut stderr)? {}
    Ok(registry.pipeline_run(&run.id)?)
}

fn wait(
    registry: &Registry,
    run_id: &str,
    interrupted: &AtomicBool,
    json: bool,
    writer: &mut impl Write,
) -> Result<u64, CliError> {
    let mut cursor = 0;
    let mut cancellation_requested = false;
    loop {
        if interrupted.load(Ordering::Relaxed) && !cancellation_requested {
            match registry.cancel_pipeline_run(run_id) {
                Ok(_) => {}
                // A command may finish between the signal and the cancellation request.
                Err(error @ pipes_core::Error::RunConflict { .. }) => {
                    if registry.pipeline_run(run_id)?.status.is_active() {
                        return Err(error.into());
                    }
                }
                Err(error) => return Err(error.into()),
            }
            cancellation_requested = true;
        }
        let run = registry.pipeline_run(run_id)?;
        let has_more = stream_page(registry, run_id, &mut cursor, json, writer)?;
        if !run.status.is_active() && !has_more {
            return Ok(cursor);
        }
        if !has_more {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

fn stream_page(
    registry: &Registry,
    run_id: &str,
    cursor: &mut u64,
    json: bool,
    writer: &mut impl Write,
) -> Result<bool, CliError> {
    let page = registry.pipeline_run_logs(run_id, *cursor)?;
    for entry in &page.entries {
        if json {
            serde_json::to_writer(
                &mut *writer,
                &OutputEvent {
                    event: "output",
                    run_id,
                    entry,
                },
            )
            .map_err(io::Error::from)?;
            writeln!(writer)?;
        } else {
            write_log(entry, writer)?;
        }
    }
    if !page.entries.is_empty() {
        writer.flush()?;
    }
    *cursor = page.next_cursor;
    Ok(page.has_more)
}

pub(crate) fn write_log(entry: &RunLogEntry, writer: &mut impl Write) -> io::Result<()> {
    let stream = match entry.stream {
        LogStream::Stdout => "stdout",
        LogStream::Stderr => "stderr",
        LogStream::System => "system",
    };
    write!(writer, "[{} {stream}", entry.sequence)?;
    if let Some(stage) = &entry.stage_id {
        write!(writer, " {stage}")?;
    }
    if let Some(command) = &entry.command_id {
        write!(writer, "/{command}")?;
    }
    write!(writer, "] {}", entry.text)?;
    if !entry.text.ends_with('\n') {
        writeln!(writer)?;
    }
    Ok(())
}

pub(crate) fn write_run(run: &PipelineRun, writer: &mut impl Write) -> io::Result<()> {
    writeln!(writer, "Run {}: {}", run.id, status_name(run.status))?;
    writeln!(writer, "App: {} ({})", run.app_name, run.app_id)?;
    writeln!(writer, "Ref: {}", run.release_ref)?;
    writeln!(writer, "Started: {}", run.started_at)?;
    if let Some(finished) = &run.finished_at {
        writeln!(writer, "Finished: {finished}")?;
    }
    writeln!(writer, "Manifest hash: {}", run.manifest.manifest_hash)?;
    writeln!(
        writer,
        "Registration updated: {}",
        run.registration_updated_at
    )?;
    for stage in &run.stages {
        writeln!(
            writer,
            "Stage {}: {}",
            stage.stage_id,
            status_name(stage.status)
        )?;
        if stage.status == RunStatus::WaitingApproval
            && let Some(manifest) = run
                .manifest
                .stages
                .iter()
                .find(|manifest| manifest.id == stage.stage_id)
        {
            if manifest.requires_approval && stage.approved_at.is_none() {
                writeln!(
                    writer,
                    "  To approve commands: pipes approve-stage {} --stage {} --approve",
                    run.id, stage.stage_id
                )?;
            } else if !manifest.promotion_criteria.is_empty()
                && run
                    .commands
                    .iter()
                    .filter(|command| command.stage_id.as_deref() == Some(stage.stage_id.as_str()))
                    .all(|command| command.status.is_success())
            {
                writeln!(
                    writer,
                    "  To approve completion: review promotion-status, then explicitly use approve-promotion for this run's release ref and registration."
                )?;
            } else if manifest.requires_approval {
                writeln!(writer, "  Local approval recorded; waiting for the runner.")?;
                // The approval response still carries its pre-approval stage message.
                continue;
            }
        }
        if let Some(message) = &stage.message {
            writeln!(writer, "  {message}")?;
        }
    }
    for command in &run.commands {
        write!(
            writer,
            "Command {}/{}: {}",
            command.stage_id.as_deref().unwrap_or("prepare"),
            command.command_id,
            status_name(command.status)
        )?;
        if let Some(code) = command.exit_code {
            write!(writer, " (exit {code})")?;
        }
        writeln!(writer)?;
        if let Some(message) = &command.message {
            writeln!(writer, "  {message}")?;
        }
    }
    if let Some(message) = &run.message {
        writeln!(writer, "{message}")?;
    }
    Ok(())
}

pub(crate) fn write_runs(runs: &[PipelineRunSummary], writer: &mut impl Write) -> io::Result<()> {
    if runs.is_empty() {
        return writeln!(writer, "No pipeline runs recorded.");
    }
    writeln!(
        writer,
        "Pipeline runs (newest 100; retained after unregistration):"
    )?;
    for run in runs {
        writeln!(
            writer,
            "{}\t{}\t{}\t{}\t{}/{} passed\t{}",
            run.id,
            run.app_id,
            run.release_ref,
            status_name(run.status),
            run.passed_commands,
            run.total_commands,
            run.started_at
        )?;
    }
    Ok(())
}

fn status_name(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Pending => "pending",
        RunStatus::Running => "running",
        RunStatus::WaitingApproval => "waiting_approval",
        RunStatus::Passed => "passed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Interrupted => "interrupted",
        RunStatus::Blocked => "blocked",
        RunStatus::Skipped => "skipped",
        RunStatus::TimedOut => "timed_out",
    }
}
