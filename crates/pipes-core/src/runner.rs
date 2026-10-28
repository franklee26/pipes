use std::{
    collections::BTreeMap,
    env, fs,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{
    CommandSummary, Error, LogStream, Parallelism, PipelineRun, Registry, Result, RunStatus,
    StartPipelineRun,
    manifest::execution_source_policy,
    registry::runs::{RunWriter, now},
    validation,
};

mod output;
mod plan;
mod process;

use output::OutputBuffer;
use plan::Plan;
use process::{Process, result_path, working_directory};

#[derive(Default)]
struct Workers {
    closed: bool,
    jobs: Vec<JoinHandle<Result<()>>>,
}

/// Owns attached workers. Dropping or shutting down the runner stops its commands.
/// Run state and output are persisted independently of this process's lifetime.
pub struct PipelineRunner {
    registry: Registry,
    stop: Arc<AtomicBool>,
    workers: Mutex<Workers>,
}

impl PipelineRunner {
    pub fn new(registry: Registry) -> Self {
        Self {
            registry,
            stop: Arc::new(AtomicBool::new(false)),
            workers: Mutex::new(Workers::default()),
        }
    }

    pub fn start(&self, request: &StartPipelineRun) -> Result<PipelineRun> {
        if !request.approved {
            return Err(Error::RunApprovalRequired);
        }
        validation::slug(&request.app_id).map_err(invalid_run)?;
        validation::manifest_hash(&request.expected_manifest_hash).map_err(invalid_run)?;
        validation::utc_timestamp(&request.expected_updated_at).map_err(invalid_run)?;
        validation::normalize_ref(&request.release_ref).map_err(invalid_run)?;
        let mut workers = self
            .workers
            .lock()
            .map_err(|_| Error::execution("runner worker lock was poisoned"))?;
        if workers.closed {
            return Err(Error::execution("the owning runner is shutting down"));
        }
        let mut index = 0;
        while index < workers.jobs.len() {
            if workers.jobs[index].is_finished() {
                workers.jobs.swap_remove(index).join().map_err(|_| {
                    Error::execution(
                        "a previous runner worker panicked; inspect its interrupted run history",
                    )
                })??;
            } else {
                index += 1;
            }
        }
        let app = self.registry.get(&request.app_id)?;
        if app.manifest.manifest_hash != request.expected_manifest_hash
            || app.updated_at != request.expected_updated_at
        {
            return Err(Error::RegistrationChanged {
                app_id: request.app_id.clone(),
            });
        }
        let require_clean = execution_source_policy(&app.manifest)?;
        let source = fs::canonicalize(&app.manifest.source_path).map_err(|error| {
            Error::execution(format!("cannot resolve registered source: {error}"))
        })?;
        if !source.is_dir() || source.to_str() != Some(app.manifest.source_path.as_str()) {
            return Err(Error::execution(
                "the registered source was moved or replaced; review and register it again",
            ));
        }
        if let Some(os) = &app.manifest.execution.os
            && os != env::consts::OS
        {
            return Err(Error::execution(format!(
                "manifest requires OS '{os}', but this host is '{}'",
                env::consts::OS
            )));
        }
        if let Some(arch) = &app.manifest.execution.arch {
            let arch = if arch == "arm64" {
                "aarch64"
            } else {
                arch.as_str()
            };
            if arch != env::consts::ARCH {
                return Err(Error::execution(format!(
                    "manifest requires architecture '{arch}', but this host is '{}'",
                    env::consts::ARCH
                )));
            }
        }
        for command in app
            .manifest
            .prepare
            .iter()
            .chain(app.manifest.stages.iter().flat_map(|stage| &stage.checks))
        {
            for (name, reference) in &command.secrets {
                let source = reference.strip_prefix("env://").ok_or_else(||
                    Error::execution(format!("command '{}' secret '{name}' uses an unsupported provider; only env:// is implemented", command.id)))?;
                let value = env::var(source).map_err(|_|
                    Error::execution(format!("command '{}' requires UTF-8 environment variable '{source}' for secret '{name}'", command.id)))?;
                if value.is_empty() || value.len() > 65536 {
                    return Err(Error::execution(format!(
                        "secret '{name}' must contain between 1 and 65536 bytes"
                    )));
                }
            }
        }
        let lock = Arc::new(
            self.registry
                .try_source_lock(&app.manifest.source_path)?
                .ok_or_else(|| Error::RunConflict {
                    message:
                        "another pipeline is already using this source directory in this registry"
                            .to_owned(),
                })?,
        );
        let max_parallel = match app.manifest.execution.max_parallel {
            Parallelism::Auto => u32::try_from(
                thread::available_parallelism()
                    .map_err(|error| {
                        Error::execution(format!("cannot determine host command capacity: {error}"))
                    })?
                    .get(),
            )
            .map_err(|error| Error::execution(format!("invalid host command capacity: {error}")))?,
            Parallelism::Limit(limit) => limit.get(),
        };
        let mut run =
            self.registry
                .create_pipeline_run(request, max_parallel, &app.manifest.source_path)?;
        let registry = self.registry.clone();
        let stop = self.stop.clone();
        let worker_lock = lock.clone();
        let worker_run = run.clone();
        let worker = thread::Builder::new()
            .name(format!("pipes-run-{}", run.id))
            .spawn(move || {
                let _lock = worker_lock;
                execute(registry, worker_run, stop, require_clean)
            });
        match worker {
            Ok(worker) => workers.jobs.push(worker),
            Err(error) => {
                finish_run(
                    &mut run,
                    RunStatus::Failed,
                    &format!("Cannot start runner worker: {error}"),
                );
                self.registry.run_writer()?.flush(&mut run, &[])?;
                return Err(Error::execution(format!(
                    "cannot start runner worker: {error}"
                )));
            }
        }
        Ok(run)
    }

    pub fn shutdown(&self) -> Result<()> {
        let jobs = {
            let mut workers = self
                .workers
                .lock()
                .map_err(|_| Error::execution("runner worker lock was poisoned"))?;
            workers.closed = true;
            self.stop.store(true, Ordering::Release);
            std::mem::take(&mut workers.jobs)
        };
        let mut failures = Vec::new();
        for job in jobs {
            match job.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(error.to_string()),
                Err(_) => failures.push(
                    "runner worker panicked; its run is interrupted, not successful".to_owned(),
                ),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(Error::execution(failures.join("; ")))
        }
    }
}

impl Drop for PipelineRunner {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            eprintln!("Pipes runner shutdown: {error}");
        }
    }
}

fn execute(
    registry: Registry,
    mut run: PipelineRun,
    stop: Arc<AtomicBool>,
    require_clean: bool,
) -> Result<()> {
    let mut writer = registry.run_writer()?;
    let result = (|| {
        if require_clean {
            check_clean_source(&mut run, &stop, &mut writer)?;
        }
        drive(&registry, &mut run, &stop, &mut writer)
    })();
    if let Err(error) = result {
        let status = if stop.load(Ordering::Acquire) {
            RunStatus::Interrupted
        } else if writer.control(&run.id)?.cancelled {
            RunStatus::Cancelled
        } else {
            RunStatus::Failed
        };
        finish_run(&mut run, status, &error.to_string());
        writer.flush(&mut run, &[]).map_err(|storage| {
            Error::execution(format!(
                "{error}; could not persist terminal run state: {storage}"
            ))
        })?;
    }
    Ok(())
}

fn check_clean_source(
    run: &mut PipelineRun,
    stop: &AtomicBool,
    writer: &mut RunWriter,
) -> Result<()> {
    run.status = RunStatus::Running;
    run.message =
        Some("Checking source.require_clean with Git before manifest commands.".to_owned());
    let mut output = OutputBuffer::default();
    output.push(
        None,
        None,
        LogStream::System,
        "Checking clean Git source (30s limit).\n",
    );
    writer.flush(run, &output.entries)?;
    if stop.load(Ordering::Acquire) || writer.control(&run.id)?.cancelled {
        return Err(Error::execution(
            "Source preflight was stopped before any manifest command ran.",
        ));
    }
    let command = CommandSummary {
        id: "source-preflight".to_owned(),
        exec: [
            "git",
            "-c",
            "core.fsmonitor=false",
            "status",
            "--porcelain",
            "--untracked-files=normal",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        cwd: None,
        timeout: Some("30s".to_owned()),
        env: BTreeMap::new(),
        secrets: BTreeMap::new(),
        result: Default::default(),
    };
    let (sender, receiver) = mpsc::sync_channel(256);
    let cwd = std::path::Path::new(&run.manifest.source_path);
    let mut process = Process::spawn(0, &command, None, run, cwd, sender, None)?;
    let mut dirty = false;
    let mut diagnostic = String::new();
    loop {
        let control = writer.control(&run.id)?;
        if stop.load(Ordering::Acquire) {
            process.terminate(
                RunStatus::Interrupted,
                "Owner closed during source preflight.",
            )?;
        } else if control.cancelled {
            process.terminate(RunStatus::Cancelled, "Cancelled during source preflight.")?;
        }
        for message in receiver.try_iter().take(256) {
            let text = process.output(message.stream, message.event)?;
            if message.stream == LogStream::Stdout && !text.is_empty() {
                dirty = true;
            } else if message.stream == LogStream::Stderr && diagnostic.len() < 4096 {
                diagnostic.extend(text.chars().take(512));
            }
        }
        if process.poll()? {
            let (status, code, message) = process.finish()?;
            if status != RunStatus::Passed {
                return Err(Error::execution(format!(
                    "Git source preflight {status:?} (exit {code:?}): {} {}. source.require_clean requires a readable Git working tree; explicitly set require_clean: false for non-Git projects.",
                    message.as_deref().unwrap_or("Git status did not succeed"),
                    diagnostic.trim(),
                )));
            }
            if dirty {
                return Err(Error::execution(
                    "source.require_clean is true, but the Git working tree has changes; commit/clean it or explicitly set require_clean: false and re-register",
                ));
            }
            run.message = None;
            output.entries.clear();
            output.push(
                None,
                None,
                LogStream::System,
                "Clean source preflight passed.\n",
            );
            return writer.flush(run, &output.entries);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn drive(
    registry: &Registry,
    run: &mut PipelineRun,
    stop: &AtomicBool,
    writer: &mut RunWriter,
) -> Result<()> {
    let plan = Plan::new(&run.manifest)?;
    let (sender, receiver) = mpsc::sync_channel::<process::OutputMessage>(256);
    let mut running = BTreeMap::<usize, Process>::new();
    let mut output = OutputBuffer::default();
    let mut gate_checks = vec![None; run.stages.len()];
    let mut stopping = None;
    let mut last_flush = Instant::now();
    loop {
        let mut changed = false;
        let control = writer.control(&run.id)?;
        if control.approvals.keys().any(|id| {
            !run.manifest
                .stages
                .iter()
                .any(|stage| stage.id == *id && stage.requires_approval)
        }) {
            return Err(Error::execution(
                "saved run control contains an invalid stage approval",
            ));
        }
        if stopping.is_none() {
            stopping = if stop.load(Ordering::Acquire) {
                Some(RunStatus::Interrupted)
            } else if control.cancelled {
                Some(RunStatus::Cancelled)
            } else {
                None
            };
        }
        for stage in &mut run.stages {
            if let Some(approved_at) = control.approvals.get(&stage.stage_id)
                && stage.approved_at.as_ref() != Some(approved_at)
            {
                stage.approved_at = Some(approved_at.clone());
                changed = true;
            }
        }
        if let Some(status) = stopping {
            let message = if status == RunStatus::Interrupted {
                "The owning desktop app or CLI is shutting down."
            } else {
                "Run cancelled by request."
            };
            for process in running.values_mut() {
                process.terminate(status, message)?;
            }
            for index in 0..run.commands.len() {
                if run.commands[index].status == RunStatus::Pending {
                    command_finished(
                        run,
                        index,
                        RunStatus::Skipped,
                        None,
                        Some(message.to_owned()),
                        &mut output,
                    );
                    changed = true;
                }
            }
        }
        for message in receiver.try_iter().take(256) {
            if let Some(process) = running.get_mut(&message.task) {
                let text = process.output(message.stream, message.event)?;
                let command = &run.commands[message.task];
                output.push(
                    command.stage_id.as_deref(),
                    Some(&command.command_id),
                    message.stream,
                    &text,
                );
            }
        }
        let mut completed = Vec::new();
        for (index, process) in &mut running {
            if process.poll()? {
                let (status, code, message) = process.finish()?;
                for (stream, text) in [
                    (LogStream::Stdout, process.stdout.push(&[], true)),
                    (LogStream::Stderr, process.stderr.push(&[], true)),
                ] {
                    output.push(
                        run.commands[*index].stage_id.as_deref(),
                        Some(&run.commands[*index].command_id),
                        stream,
                        &text,
                    );
                }
                command_finished(run, *index, status, code, message, &mut output);
                completed.push(*index);
                changed = true;
            }
        }
        for index in completed {
            running.remove(&index);
        }
        if let Some(status) = stopping {
            if running.is_empty() {
                let message = if status == RunStatus::Interrupted {
                    "Owning process closed; active commands stopped. This run will not resume automatically."
                } else {
                    "Run cancelled by request."
                };
                finish_run(run, status, message);
                writer.flush(run, &output.entries)?;
                return Ok(());
            }
        } else {
            changed |= advance_stages(writer, run, &plan, &mut gate_checks, &mut output);
            let mut barriers = vec![None; plan.barriers.len()];
            for (index, task) in plan.tasks.iter().enumerate() {
                if run.commands[index].status != RunStatus::Pending {
                    continue;
                }
                let prerequisite = plan.prerequisite(index, &run.commands, &mut barriers);
                if task
                    .stage
                    .is_some_and(|stage| !run.stages[stage].status.is_active())
                    || (!prerequisite.is_active() && !prerequisite.is_success())
                {
                    command_finished(
                        run,
                        index,
                        RunStatus::Skipped,
                        None,
                        Some("A required predecessor did not pass.".to_owned()),
                        &mut output,
                    );
                    changed = true;
                    continue;
                }
                if running.len() >= run.max_parallel as usize
                    || task
                        .stage
                        .is_some_and(|stage| run.stages[stage].status != RunStatus::Running)
                    || !prerequisite.is_success()
                    || task.budgets.iter().any(|budget| {
                        running
                            .keys()
                            .filter(|active| plan.tasks[**active].budgets.contains(budget))
                            .count()
                            >= plan.budgets[*budget]
                    })
                {
                    continue;
                }
                let cwd = match working_directory(&run.manifest.source_path, &task.command) {
                    Ok(cwd) => cwd,
                    Err(error) => {
                        command_finished(
                            run,
                            index,
                            RunStatus::Failed,
                            None,
                            Some(error.to_string()),
                            &mut output,
                        );
                        changed = true;
                        continue;
                    }
                };
                let path = match result_path(&task.command, &cwd) {
                    Ok(path) => path,
                    Err(error) => {
                        command_finished(
                            run,
                            index,
                            RunStatus::Failed,
                            None,
                            Some(error.to_string()),
                            &mut output,
                        );
                        changed = true;
                        continue;
                    }
                };
                if path.is_some() && running.values().any(|active| active.result_path == path) {
                    continue;
                }
                let result_lock = if let Some(path) = &path {
                    match registry.try_result_lock(path)? {
                        Some(lock) => Some(lock),
                        None => continue,
                    }
                } else {
                    None
                };
                let stage = task
                    .stage
                    .map(|stage| run.manifest.stages[stage].id.as_str());
                output.push(
                    stage,
                    Some(&task.command.id),
                    LogStream::System,
                    &format!("Starting command in {}.\n", cwd.display()),
                );
                run.commands[index].status = RunStatus::Running;
                run.commands[index].started_at = Some(now());
                match Process::spawn(
                    index,
                    &task.command,
                    stage,
                    run,
                    &cwd,
                    sender.clone(),
                    result_lock,
                ) {
                    Ok(process) => {
                        running.insert(index, process);
                    }
                    Err(error) => command_finished(
                        run,
                        index,
                        RunStatus::Failed,
                        None,
                        Some(error.to_string()),
                        &mut output,
                    ),
                }
                changed = true;
            }
            if running.is_empty()
                && run.stages.iter().all(|stage| !stage.status.is_active())
                && run
                    .commands
                    .iter()
                    .all(|command| !command.status.is_active())
            {
                let passed = run.stages.iter().all(|stage| stage.status.is_success())
                    && run
                        .commands
                        .iter()
                        .all(|command| command.status.is_success());
                run.status = if passed {
                    RunStatus::Passed
                } else {
                    RunStatus::Failed
                };
                run.finished_at = Some(now());
                run.message = (!passed).then(|| {
                    "One or more required commands or stage gates did not pass.".to_owned()
                });
                writer.flush(run, &output.entries)?;
                return Ok(());
            }
        }
        let status = if running.is_empty()
            && run
                .stages
                .iter()
                .any(|stage| stage.status == RunStatus::WaitingApproval)
        {
            RunStatus::WaitingApproval
        } else {
            RunStatus::Running
        };
        if status != run.status {
            run.status = status;
            changed = true;
        }
        if changed
            || (!output.entries.is_empty() && last_flush.elapsed() >= Duration::from_millis(40))
        {
            writer.flush(run, &output.entries)?;
            output.entries.clear();
            last_flush = Instant::now();
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn advance_stages(
    writer: &mut RunWriter,
    run: &mut PipelineRun,
    plan: &Plan,
    gate_checks: &mut [Option<Instant>],
    output: &mut OutputBuffer,
) -> bool {
    let mut changed = false;
    let prep_failed = run.commands[..plan.prepare_count]
        .iter()
        .any(|command| !command.status.is_active() && !command.status.is_success());
    let prep_ready = run.commands[..plan.prepare_count]
        .iter()
        .all(|command| command.status.is_success());
    for index in &plan.stage_order {
        let index = *index;
        if !run.stages[index].status.is_active() {
            continue;
        }
        if prep_failed
            || plan.stage_parents[index].iter().any(|parent| {
                let status = run.stages[*parent].status;
                !status.is_active() && !status.is_success()
            })
        {
            changed |= set_stage(
                run,
                index,
                RunStatus::Blocked,
                Some("A required preparation or parent stage did not pass.".to_owned()),
                output,
            );
            continue;
        }
        if !prep_ready
            || plan.stage_parents[index]
                .iter()
                .any(|parent| !run.stages[*parent].status.is_success())
        {
            continue;
        }
        if run.manifest.stages[index].requires_approval && run.stages[index].approved_at.is_none() {
            changed |= set_stage(
                run,
                index,
                RunStatus::WaitingApproval,
                Some(
                    "Local execution approval is required before this stage's commands run."
                        .to_owned(),
                ),
                output,
            );
            continue;
        }
        let tasks = &plan.stage_tasks[index];
        if tasks
            .iter()
            .any(|task| run.commands[*task].status.is_active())
        {
            changed |= set_stage(run, index, RunStatus::Running, None, output);
            continue;
        }
        if let Some(failed) = tasks
            .iter()
            .find(|task| !run.commands[**task].status.is_success())
        {
            changed |= set_stage(
                run,
                index,
                RunStatus::Failed,
                run.commands[*failed].message.clone(),
                output,
            );
            continue;
        }
        if run.stages[index].started_at.is_none() {
            changed |= set_stage(run, index, RunStatus::Running, None, output);
        }
        if !run.manifest.stages[index].promotion_criteria.is_empty() {
            if gate_checks[index]
                .is_some_and(|checked| checked.elapsed() < Duration::from_millis(500))
            {
                continue;
            }
            gate_checks[index] = Some(Instant::now());
            match writer.promotion_gate(run, index) {
                Ok((true, _)) => {
                    changed = true;
                }
                Ok((false, message)) => {
                    changed |= set_stage(
                        run,
                        index,
                        RunStatus::WaitingApproval,
                        Some(message),
                        output,
                    );
                    continue;
                }
                Err(error) => {
                    changed |= set_stage(
                        run,
                        index,
                        RunStatus::Failed,
                        Some(error.to_string()),
                        output,
                    );
                    continue;
                }
            }
        }
        changed |= set_stage(run, index, RunStatus::Passed, None, output);
    }
    changed
}

fn set_stage(
    run: &mut PipelineRun,
    index: usize,
    status: RunStatus,
    message: Option<String>,
    output: &mut OutputBuffer,
) -> bool {
    let stage = &mut run.stages[index];
    if stage.status == status && stage.message == message {
        return false;
    }
    stage.status = status;
    stage.message = message;
    if status == RunStatus::Running && stage.started_at.is_none() {
        stage.started_at = Some(now());
    }
    if !status.is_active() {
        stage.finished_at = Some(now());
    }
    output.push(
        Some(&stage.stage_id),
        None,
        LogStream::System,
        &format!(
            "Stage {status:?}. {}\n",
            stage.message.as_deref().unwrap_or("")
        ),
    );
    true
}

fn command_finished(
    run: &mut PipelineRun,
    index: usize,
    status: RunStatus,
    code: Option<i32>,
    message: Option<String>,
    output: &mut OutputBuffer,
) {
    let command = &mut run.commands[index];
    command.status = status;
    command.exit_code = code;
    command.message = message;
    command.finished_at = Some(now());
    output.push(
        command.stage_id.as_deref(),
        Some(&command.command_id),
        LogStream::System,
        &format!(
            "Command {status:?} / exit {}. {}\n",
            code.map_or_else(|| "none".to_owned(), |code| code.to_string()),
            command.message.as_deref().unwrap_or("")
        ),
    );
}

fn finish_run(run: &mut PipelineRun, status: RunStatus, message: &str) {
    run.status = status;
    run.finished_at = Some(now());
    run.message = Some(message.to_owned());
    for command in &mut run.commands {
        if command.status.is_active() {
            command.status = if command.status == RunStatus::Pending {
                RunStatus::Skipped
            } else {
                status
            };
            command.finished_at = run.finished_at.clone();
            command.message = Some(message.to_owned());
        }
    }
    for stage in &mut run.stages {
        if stage.status.is_active() {
            stage.status = if status == RunStatus::Failed && stage.status == RunStatus::Pending {
                RunStatus::Blocked
            } else {
                status
            };
            stage.finished_at = run.finished_at.clone();
            stage.message = Some(message.to_owned());
        }
    }
}

fn invalid_run(message: impl ToString) -> Error {
    Error::InvalidRun {
        message: message.to_string(),
    }
}
