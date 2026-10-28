use std::{
    env,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use command_group::{CommandGroup, GroupChild};

use super::output::TextStream;
use crate::{
    CommandResult, CommandSummary, Error, LogStream, PipelineRun, Result, RunStatus,
    registry::runs::ExecutionLock, results::ResultProbe,
};

pub(super) enum OutputEvent {
    Data(Vec<u8>),
    Error(String),
    Closed,
}

pub(super) struct OutputMessage {
    pub task: usize,
    pub stream: LogStream,
    pub event: OutputEvent,
}

pub(super) struct Process {
    child: GroupChild,
    pub result_path: Option<PathBuf>,
    probe: Option<ResultProbe>,
    pub stdout: TextStream,
    pub stderr: TextStream,
    readers: Vec<JoinHandle<()>>,
    reader_stop: Arc<AtomicBool>,
    closed: usize,
    started: Instant,
    timeout: Option<Duration>,
    exited_at: Option<Instant>,
    exit: Option<ExitStatus>,
    forced: Option<(RunStatus, String)>,
    group_stopped: bool,
    _result_lock: Option<ExecutionLock>,
}

pub(super) fn working_directory(source: &str, command: &CommandSummary) -> Result<PathBuf> {
    let path = Path::new(source).join(command.cwd.as_deref().unwrap_or("."));
    let path = path.canonicalize().map_err(|error| {
        Error::execution(format!(
            "cannot resolve command '{}' working directory '{}': {error}",
            command.id,
            path.display()
        ))
    })?;
    if !path.is_dir() {
        return Err(Error::execution(format!(
            "command '{}' working directory is not a directory",
            command.id
        )));
    }
    Ok(path)
}

pub(super) fn result_path(command: &CommandSummary, cwd: &Path) -> Result<Option<PathBuf>> {
    let result: &CommandResult = &command.result;
    match result {
        CommandResult::ExitCode { .. } => Ok(None),
        CommandResult::JsonFile { path, .. } => {
            let path = cwd.join(path);
            let mut normalized = PathBuf::new();
            for component in path.components() {
                match component {
                    std::path::Component::CurDir => {}
                    std::path::Component::ParentDir => {
                        normalized.pop();
                    }
                    component => normalized.push(component),
                }
            }
            if let Some((parent, name)) = normalized.parent().zip(normalized.file_name()) {
                match parent.canonicalize() {
                    Ok(parent) => return Ok(Some(parent.join(name))),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(Error::execution(format!(
                            "cannot resolve result file directory: {error}"
                        )));
                    }
                }
            }
            Ok(Some(normalized))
        }
    }
}

impl Process {
    pub fn spawn(
        task: usize,
        command: &CommandSummary,
        stage: Option<&str>,
        run: &PipelineRun,
        cwd: &Path,
        output: SyncSender<OutputMessage>,
        result_lock: Option<ExecutionLock>,
    ) -> Result<Self> {
        let probe = ResultProbe::capture(&command.result, cwd)?;
        let mut secrets = Vec::new();
        let program = Path::new(&command.exec[0]);
        let program = if program.components().count() > 1 {
            cwd.join(program)
        } else {
            program.to_owned()
        };
        let mut builder = Command::new(program);
        builder
            .args(&command.exec[1..])
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&command.env);
        for (name, reference) in &command.secrets {
            let source = reference.strip_prefix("env://").ok_or_else(|| {
                Error::execution(format!(
                    "secret '{name}' uses an unsupported provider; only env:// references can run"
                ))
            })?;
            let value = env::var(source).map_err(|_| {
                Error::execution(format!(
                    "secret '{name}' requires a UTF-8 environment variable '{source}'"
                ))
            })?;
            if value.is_empty() || value.len() > 65536 {
                return Err(Error::execution(format!(
                    "secret '{name}' must contain between 1 and 65536 bytes"
                )));
            }
            builder.env(name, &value);
            secrets.push(value);
        }
        builder
            .env("PIPES_RUN_ID", &run.id)
            .env("PIPES_RELEASE_REF", &run.release_ref)
            .env("PIPES_STAGE_ID", stage.unwrap_or("prepare"))
            .env("PIPES_COMMAND_ID", &command.id)
            .env_remove("PIPES_RESULT_FILE");
        if let Some(path) = probe.path() {
            builder.env("PIPES_RESULT_FILE", path);
        }
        let timeout = command
            .timeout
            .as_deref()
            .or(run.manifest.execution.timeout.as_deref())
            .map(humantime::parse_duration)
            .transpose()
            .map_err(|error| Error::execution(format!("invalid saved command timeout: {error}")))?;
        let result_path = result_path(command, cwd)?;
        let child = builder.group_spawn().map_err(|error| {
            Error::execution(format!("cannot start command '{}': {error}", command.id))
        })?;
        let mut process = Self {
            child,
            result_path,
            probe: Some(probe),
            stdout: TextStream::new(secrets.clone()),
            stderr: TextStream::new(secrets),
            readers: Vec::new(),
            reader_stop: Arc::new(AtomicBool::new(false)),
            closed: 0,
            started: Instant::now(),
            timeout,
            exited_at: None,
            exit: None,
            forced: None,
            group_stopped: false,
            _result_lock: result_lock,
        };
        let stdout = process
            .child
            .inner()
            .stdout
            .take()
            .ok_or_else(|| Error::execution("command stdout pipe is missing"))?;
        let stderr = process
            .child
            .inner()
            .stderr
            .take()
            .ok_or_else(|| Error::execution("command stderr pipe is missing"))?;
        process.readers.push(spawn_reader(
            stdout,
            task,
            LogStream::Stdout,
            output.clone(),
            process.reader_stop.clone(),
        )?);
        process.readers.push(spawn_reader(
            stderr,
            task,
            LogStream::Stderr,
            output,
            process.reader_stop.clone(),
        )?);
        Ok(process)
    }

    pub fn output(&mut self, stream: LogStream, event: OutputEvent) -> Result<String> {
        let text = match stream {
            LogStream::Stdout => &mut self.stdout,
            LogStream::Stderr => &mut self.stderr,
            LogStream::System => return Err(Error::execution("unexpected process output stream")),
        };
        match event {
            OutputEvent::Data(bytes) => Ok(text.push(&bytes, false)),
            OutputEvent::Closed => {
                self.closed += 1;
                Ok(text.push(&[], true))
            }
            OutputEvent::Error(message) => {
                self.terminate(
                    RunStatus::Failed,
                    &format!("Cannot capture command output: {message}"),
                )?;
                Ok(String::new())
            }
        }
    }

    pub fn terminate(&mut self, status: RunStatus, message: &str) -> Result<()> {
        if self.forced.is_none() {
            self.forced = Some((status, message.to_owned()));
        }
        self.stop_group()
    }

    fn stop_group(&mut self) -> Result<()> {
        if !self.group_stopped {
            match self.child.kill() {
                Ok(()) => {}
                Err(error) if group_gone(&error) => {}
                Err(error) => {
                    return Err(Error::execution(format!(
                        "cannot stop command process group: {error}"
                    )));
                }
            }
            self.group_stopped = true;
        }
        Ok(())
    }

    pub fn poll(&mut self) -> Result<bool> {
        if self.exit.is_none() {
            self.exit = self.child.try_wait().map_err(|error| {
                Error::execution(format!("cannot collect command exit status: {error}"))
            })?;
            if self.exit.is_some() {
                self.exited_at = Some(Instant::now());
                // A command cannot leave background children holding our output pipes open.
                self.stop_group()?;
            } else if self.forced.is_none()
                && self
                    .timeout
                    .is_some_and(|timeout| self.started.elapsed() >= timeout)
            {
                self.terminate(
                    RunStatus::TimedOut,
                    "Command exceeded its configured timeout.",
                )?;
            }
        }
        if self.exit.is_some() && self.closed == 2 {
            return Ok(true);
        }
        if self
            .exited_at
            .is_some_and(|time| time.elapsed() >= Duration::from_secs(2))
        {
            if self.forced.is_none() {
                self.forced = Some((RunStatus::Failed, "A detached descendant kept the output pipes open after the command exited; capture was stopped.".to_owned()));
            }
            self.reader_stop.store(true, Ordering::Release);
            return Ok(true);
        }
        Ok(false)
    }

    pub fn finish(&mut self) -> Result<(RunStatus, Option<i32>, Option<String>)> {
        self.reader_stop.store(true, Ordering::Release);
        self.join_readers()?;
        let code = self.exit.and_then(|status| status.code());
        if let Some((status, message)) = self.forced.take() {
            return Ok((status, code, Some(message)));
        }
        let probe = self
            .probe
            .take()
            .ok_or_else(|| Error::execution("command result probe is missing"))?;
        match probe.evaluate(code) {
            Ok(true) => Ok((RunStatus::Passed, code, None)),
            Ok(false) => Ok((
                RunStatus::Failed,
                code,
                Some("Command reported failure using its configured result contract.".to_owned()),
            )),
            Err(error) => Ok((RunStatus::Failed, code, Some(error.to_string()))),
        }
    }

    fn join_readers(&mut self) -> Result<()> {
        let mut failed = false;
        for reader in self.readers.drain(..) {
            if reader.join().is_err() {
                failed = true;
            }
        }
        if failed {
            Err(Error::execution("a command output reader panicked"))
        } else {
            Ok(())
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Err(error) = self.stop_group() {
            eprintln!("Pipes runner cleanup: {error}");
        } else if let Err(error) = self.child.wait() {
            eprintln!("Pipes runner could not reap command: {error}");
        }
        self.reader_stop.store(true, Ordering::Release);
        if let Err(error) = self.join_readers() {
            eprintln!("Pipes runner cleanup: {error}");
        }
    }
}

fn group_gone(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
    ) {
        return true;
    }
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::ESRCH) {
        return true;
    }
    false
}

#[cfg(unix)]
trait OutputPipe: Read + std::os::fd::AsRawFd + Send {}
#[cfg(unix)]
impl<T: Read + std::os::fd::AsRawFd + Send> OutputPipe for T {}
#[cfg(not(unix))]
trait OutputPipe: Read + Send {}
#[cfg(not(unix))]
impl<T: Read + Send> OutputPipe for T {}

fn spawn_reader(
    mut pipe: impl OutputPipe + 'static,
    task: usize,
    stream: LogStream,
    sender: SyncSender<OutputMessage>,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<()>> {
    #[cfg(unix)]
    {
        let fd = pipe.as_raw_fd();
        // The owned pipe remains open through both fcntl calls and the reader's lifetime.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            return Err(Error::execution(format!(
                "cannot configure output pipe: {}",
                io::Error::last_os_error()
            )));
        }
    }
    thread::Builder::new()
        .name(format!("pipes-output-{task}-{stream:?}"))
        .spawn(move || {
            let send = |event| {
                send_output(
                    &sender,
                    &stop,
                    OutputMessage {
                        task,
                        stream,
                        event,
                    },
                )
            };
            let mut bytes = [0; 4096];
            while !stop.load(Ordering::Acquire) {
                match pipe.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(length) => {
                        if !send(OutputEvent::Data(bytes[..length].to_vec())) {
                            return;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => {
                        send(OutputEvent::Error(error.to_string()));
                        break;
                    }
                }
            }
            send(OutputEvent::Closed);
        })
        .map_err(|error| Error::execution(format!("cannot start output reader: {error}")))
}

fn send_output(
    sender: &SyncSender<OutputMessage>,
    stop: &AtomicBool,
    mut message: OutputMessage,
) -> bool {
    loop {
        match sender.try_send(message) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(returned)) => {
                if stop.load(Ordering::Acquire) {
                    return false;
                }
                message = returned;
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}
