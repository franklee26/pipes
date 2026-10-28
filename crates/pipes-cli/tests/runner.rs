#![cfg(unix)]

use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tempfile::{Builder, TempDir};

const RUN: &[&str] = &["run", "demo", "--release-ref", "v1", "--approve"];
const JSON_RESULT: &str =
    "{type: json-file, path: status.json, pointer: /status, success: passed, failure: failed}";

struct Fixture {
    root: TempDir,
    state: PathBuf,
}

impl Fixture {
    fn new(stages: &str) -> Self {
        let root = Builder::new()
            .prefix(".pipes-cli-run-test-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap();
        fs::create_dir(root.path().join("source")).unwrap();
        fs::write(
            root.path().join("app.yaml"),
            format!(
                "api: pipes/v1\napp: demo\nsource: {{path: source, snapshot: working-tree, require_clean: false}}\nversion: {{from: 'file:VERSION'}}\nexecution: {{timeout: 10s}}\n{stages}"
            ),
        )
        .unwrap();
        let state = root.path().join("state");
        Self { root, state }
    }

    fn shell(script: &str) -> Self {
        Self::new(&format!(
            "stages:\n  - id: build\n    checks:\n      - id: check\n        exec: {}\n",
            json!(["sh", "-c", script])
        ))
    }

    fn with_result(self) -> Self {
        let path = self.root.path().join("app.yaml");
        let yaml = fs::read_to_string(&path).unwrap();
        fs::write(path, format!("{yaml}        result: {JSON_RESULT}\n")).unwrap();
        self
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pipes"));
        command
            .current_dir(self.root.path())
            .env_remove("PIPES_DATA_DIR")
            .arg("--data-dir")
            .arg(&self.state);
        command
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self.command().arg("--json").args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn error(&self, args: &[&str], code: &str) {
        let output = self.command().arg("--json").args(args).output().unwrap();
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"]["code"], code, "{args:?}: {error}");
        assert!(error["error"]["message"].as_str().unwrap().len() > 10);
    }

    fn register(&self) -> Value {
        self.json(&["register", "app.yaml"])
    }

    fn start(&self, args: &[&str], json: bool) -> Running {
        let mut command = self.command();
        if json {
            command.arg("--json");
        }
        command.args(args);
        Running::spawn(command)
    }

    fn wait_status(&self, run_id: &str, status: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let run = self.json(&["run-status", run_id]);
            if run["status"] == status {
                return run;
            }
            assert!(Instant::now() < deadline, "waiting for {status}: {run}");
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn logs(&self, run_id: &str) -> Vec<Value> {
        let mut entries = Vec::new();
        let mut cursor = 0;
        loop {
            let page = self.json(&["logs", run_id, "--after", &cursor.to_string()]);
            let batch = page["entries"].as_array().unwrap();
            assert!(batch.len() <= 256);
            assert!(
                batch
                    .iter()
                    .all(|entry| entry["sequence"].as_u64().unwrap() > cursor)
            );
            entries.extend(batch.iter().cloned());
            let next = page["nextCursor"].as_u64().unwrap();
            assert!(next >= cursor);
            if !page["hasMore"].as_bool().unwrap() {
                return entries;
            }
            assert!(next > cursor);
            cursor = next;
        }
    }
}

struct Running {
    child: Child,
    stdout: Option<JoinHandle<Vec<u8>>>,
    stderr: Option<JoinHandle<Vec<u8>>>,
    lines: Receiver<Vec<u8>>,
}

impl Running {
    fn spawn(mut command: Command) -> Self {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let stdout = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let stderr = child.stderr.take().unwrap();
        let (sender, lines) = mpsc::channel();
        let stderr = thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut bytes = Vec::new();
            loop {
                let mut line = Vec::new();
                if reader.read_until(b'\n', &mut line).unwrap() == 0 {
                    break;
                }
                bytes.extend_from_slice(&line);
                let _ = sender.send(line);
            }
            bytes
        });
        Self {
            child,
            stdout: Some(stdout),
            stderr: Some(stderr),
            lines,
        }
    }

    fn wait_output(&self, text: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let line = self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| panic!("waiting for {text:?}: {error}"));
            let event: Value = serde_json::from_slice(&line).unwrap();
            assert_eq!(event["event"], "output", "{event}");
            if event["entry"]["text"].as_str().unwrap().contains(text) {
                return event;
            }
        }
    }

    fn interrupt(&self) {
        assert!(
            Command::new("kill")
                .args(["-INT", &self.child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
    }

    fn finish(mut self) -> Output {
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "owning CLI did not finish");
            thread::sleep(Duration::from_millis(20));
        };
        Output {
            status,
            stdout: self.stdout.take().unwrap().join().unwrap(),
            stderr: self.stderr.take().unwrap().join().unwrap(),
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        let _ = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .output();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn final_run(output: &Output, status: &str) -> Value {
    assert_eq!(
        output.status.success(),
        status == "passed",
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout.split(|byte| *byte == b'\n').count(), 2);
    let run: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(run["status"], status, "{run}");
    assert!(run["finishedAt"].is_string());
    for line in output.stderr.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let event: Value = serde_json::from_slice(line).unwrap();
        assert_eq!(event["event"], "output", "{event}");
        assert_eq!(event["runId"], run["id"], "{event}");
        assert!(event["entry"]["sequence"].as_u64().unwrap() > 0);
    }
    run
}

#[test]
fn partial_output_is_live_and_json_stdout_is_one_final_run_with_durable_history() {
    let fixture = Fixture::shell(
        "printf partial; printf warning >&2; while [ ! -f continue ]; do sleep 0.02; done; printf '\\nfinished\\n'",
    );
    let registered = fixture.register();
    let mut running = fixture.start(RUN, true);
    let event = running.wait_output("partial");
    let id = event["runId"].as_str().unwrap();
    assert!(running.child.try_wait().unwrap().is_none());
    assert_eq!(event["entry"]["stream"], "stdout");
    assert_eq!(event["entry"]["commandId"], "check");
    assert_eq!(event["entry"]["text"], "partial");
    let active = fixture.json(&["run-status", id]);
    assert_eq!(active["status"], "running");
    let summaries = fixture.json(&["runs", "demo"]);
    assert_eq!(summaries[0]["id"], id);
    assert!(summaries[0].get("manifest").is_none());
    let partial = fixture.json(&["logs", id]);
    assert!(
        partial["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| { entry["text"] == "partial" && entry["stream"] == "stdout" })
    );
    let cursor = partial["nextCursor"].as_u64().unwrap();
    let later = fixture.json(&["logs", id, "--after", &cursor.to_string()]);
    assert!(
        later["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["sequence"].as_u64().unwrap() > cursor)
    );
    let unregistered = fixture
        .command()
        .args(["unregister", "demo"])
        .output()
        .unwrap();
    assert!(unregistered.status.success());
    assert!(unregistered.stderr.is_empty());
    assert!(
        String::from_utf8(unregistered.stdout)
            .unwrap()
            .contains("Active runs continue using their frozen snapshots")
    );
    assert_eq!(fixture.json(&["list"]), json!([]));
    assert_eq!(fixture.json(&["run-status", id])["status"], "running");
    fs::write(fixture.root.path().join("source/continue"), "").unwrap();
    let output = running.finish();
    let run = final_run(&output, "passed");
    assert_eq!(run["manifest"], registered["manifest"]);
    let logs = fixture.logs(id);
    let streamed: Vec<Value> = String::from_utf8(output.stderr)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["entry"].clone())
        .collect();
    assert_eq!(streamed, logs);
    let text = |stream: &str| {
        logs.iter()
            .filter(|entry| entry["stream"] == stream)
            .map(|entry| entry["text"].as_str().unwrap())
            .collect::<String>()
    };
    assert_eq!(text("stdout"), "partial\nfinished\n");
    assert_eq!(text("stderr"), "warning");
    let last = logs.last().unwrap()["sequence"].as_u64().unwrap();
    assert_eq!(
        fixture.json(&["logs", id, "--after", &last.to_string()]),
        json!({"entries": [], "nextCursor": last, "hasMore": false})
    );
    assert_eq!(fixture.json(&["run-status", id]), run);
    assert_eq!(fixture.json(&["runs"])[0]["id"], id);
    assert_eq!(fixture.json(&["runs", "demo"])[0]["status"], "passed");
    assert_eq!(fixture.json(&["runs", "other"]), json!([]));
    assert_eq!(fixture.logs(id), logs);
}

#[test]
fn final_exit_status_follows_exit_code_and_json_file_contracts() {
    for (script, json_result, status) in [
        ("printf ok", false, "passed"),
        ("printf bad >&2; exit 7", false, "failed"),
        (
            "printf '{\"status\":\"passed\"}' > status.json",
            true,
            "passed",
        ),
        (
            "printf '{\"status\":\"failed\"}' > status.json",
            true,
            "failed",
        ),
    ] {
        let fixture = Fixture::shell(script);
        let fixture = if json_result {
            fixture.with_result()
        } else {
            fixture
        };
        fixture.register();
        let output = fixture.start(RUN, true).finish();
        let run = final_run(&output, status);
        assert_eq!(run["commands"][0]["status"], status);
        if script.contains("exit 7") {
            assert_eq!(run["commands"][0]["exitCode"], 7);
        }
        let id = run["id"].as_str().unwrap();
        assert_eq!(fixture.json(&["run-status", id]), run);
        assert_eq!(fixture.json(&["runs"])[0]["status"], status);
    }
}

#[test]
fn execution_consent_requires_approve_and_a_current_paired_review_token() {
    let fixture = Fixture::shell("printf consent > approved-marker");
    let registered = fixture.register();
    let hash = registered["manifest"]["manifestHash"].as_str().unwrap();
    let updated_at = registered["updatedAt"].as_str().unwrap();
    fixture.error(&["run", "demo", "--release-ref", "v1"], "usage");
    fixture.error(&["run", "demo", "--approve"], "usage");
    fixture.error(&[RUN, &["--expected-hash", hash]].concat(), "usage");
    fixture.error(
        &[RUN, &["--expected-updated-at", updated_at]].concat(),
        "usage",
    );
    fixture.error(
        &[
            RUN,
            &[
                "--expected-hash",
                &"0".repeat(64),
                "--expected-updated-at",
                updated_at,
            ],
        ]
        .concat(),
        "registration_changed",
    );
    fixture.error(
        &[
            RUN,
            &[
                "--expected-hash",
                hash,
                "--expected-updated-at",
                "2000-01-01T00:00:00Z",
            ],
        ]
        .concat(),
        "registration_changed",
    );
    fixture.error(
        &["run", "demo", "--release-ref", "", "--approve"],
        "invalid_run",
    );
    assert_eq!(fixture.json(&["runs"]), json!([]));
    assert!(!fixture.root.path().join("source/approved-marker").exists());
    fs::remove_file(fixture.root.path().join("app.yaml")).unwrap();
    let output = fixture
        .start(
            &[
                RUN,
                &["--expected-hash", hash, "--expected-updated-at", updated_at],
            ]
            .concat(),
            true,
        )
        .finish();
    final_run(&output, "passed");
    assert!(fixture.root.path().join("source/approved-marker").is_file());
}

#[test]
fn separate_cli_stage_approval_releases_only_the_requested_waiting_gate() {
    let fixture = Fixture::new(
        "stages:
  - id: build
    checks:
      - {id: first, exec: [printf, before]}
  - id: release
    require: [approval.local]
    checks:
      - {id: deploy, exec: [touch, approved-marker]}
",
    );
    fixture.register();
    let mut running = fixture.start(RUN, true);
    let event = running.wait_output("before");
    let id = event["runId"].as_str().unwrap();
    let waiting = fixture.wait_status(id, "waiting_approval");
    assert!(running.child.try_wait().unwrap().is_none());
    assert_eq!(waiting["stages"][1]["status"], "waiting_approval");
    assert_eq!(waiting["stages"][1]["approvedAt"], Value::Null);
    assert!(!fixture.root.path().join("source/approved-marker").exists());
    fixture.error(&["approve-stage", id, "--stage", "release"], "usage");
    fixture.error(
        &["approve-stage", id, "--stage", "unknown", "--approve"],
        "stage_not_found",
    );
    fixture.error(
        &["approve-stage", id, "--stage", "build", "--approve"],
        "run_conflict",
    );
    assert_eq!(
        fixture.json(&["run-status", id])["status"],
        "waiting_approval"
    );
    fixture.json(&["approve-stage", id, "--stage", "release", "--approve"]);
    let output = running.finish();
    let run = final_run(&output, "passed");
    assert!(run["stages"][1]["approvedAt"].is_string());
    assert!(fixture.root.path().join("source/approved-marker").is_file());
    fixture.error(
        &["approve-stage", id, "--stage", "release", "--approve"],
        "run_conflict",
    );
}

#[test]
fn cancellation_and_ctrl_c_wait_for_cleanup_and_return_a_nonzero_final_run() {
    for signal in [false, true] {
        let fixture =
            Fixture::shell("printf ready; (sleep 3; printf leaked > orphan-marker) & wait");
        fixture.register();
        let running = fixture.start(RUN, true);
        let event = running.wait_output("ready");
        let id = event["runId"].as_str().unwrap();
        if signal {
            running.interrupt();
        } else {
            let requested = fixture.json(&["cancel-run", id]);
            assert_eq!(requested["id"], id);
        }
        let output = running.finish();
        let run = final_run(&output, "cancelled");
        assert_eq!(run["commands"][0]["status"], "cancelled");
        assert_eq!(fixture.json(&["run-status", id]), run);
        fixture.error(&["cancel-run", id], "run_conflict");
        thread::sleep(Duration::from_millis(3100));
        assert!(!fixture.root.path().join("source/orphan-marker").exists());
    }
}

#[test]
fn both_large_output_pipes_are_drained_while_stdout_remains_structured() {
    let fixture = Fixture::shell(
        "i=0; while [ \"$i\" -lt 4000 ]; do printf 'stdout-abcdefghijklmnopqrstuvwxyz0123456789\\n'; printf 'stderr-abcdefghijklmnopqrstuvwxyz0123456789\\n' >&2; i=$((i + 1)); done; printf stdout-end; printf stderr-end >&2",
    );
    fixture.register();
    let output = fixture.start(RUN, true).finish();
    let run = final_run(&output, "passed");
    let entries = fixture.logs(run["id"].as_str().unwrap());
    for (stream, ending) in [("stdout", "stdout-end"), ("stderr", "stderr-end")] {
        let text = entries
            .iter()
            .filter(|entry| entry["stream"] == stream)
            .map(|entry| entry["text"].as_str().unwrap())
            .collect::<String>();
        assert!(text.ends_with(ending), "{stream}: {} bytes", text.len());
        assert!(text.len() > 64 * 1024);
    }
}

#[test]
fn run_history_errors_keep_stable_json_codes_and_validate_log_cursors() {
    let fixture = Fixture::shell("true");
    assert_eq!(fixture.json(&["runs"]), json!([]));
    for args in [
        vec!["run-status", "99999"],
        vec!["logs", "99999"],
        vec!["cancel-run", "99999"],
        vec!["approve-stage", "99999", "--stage", "build", "--approve"],
    ] {
        fixture.error(&args, "run_not_found");
    }
    fixture.error(&["logs", "99999", "--after", "nope"], "usage");
    fixture.error(&["logs", "99999", "--after", "-1"], "usage");
    fixture.error(
        &["logs", "99999", "--after", "9007199254740992"],
        "invalid_run",
    );
    fixture.error(&["run-status", "not-an-id"], "invalid_run");
}

#[test]
fn human_inspection_shows_results_and_live_output_stays_off_summary_stdout() {
    let fixture =
        Fixture::shell("printf human-live; printf '{\"status\":\"passed\"}' > status.json")
            .with_result();
    let path = fixture.root.path().join("app.yaml");
    let yaml = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        format!("{yaml}prepare:\n  - {{id: setup, exec: [printf, setup-live]}}\n"),
    )
    .unwrap();
    fixture.register();
    let inspection = fixture
        .command()
        .args(["inspect", "demo"])
        .output()
        .unwrap();
    assert!(inspection.status.success());
    let text = String::from_utf8(inspection.stdout).unwrap();
    assert!(text.contains("setup: exit-code; success codes: 0"));
    assert!(text.contains("check: json-file status.json; pointer \"/status\""));
    assert!(text.contains("success: \"passed\"; failure: \"failed\""));
    let output = fixture.start(RUN, false).finish();
    assert!(output.status.success(), "{output:?}");
    let summary = String::from_utf8(output.stdout).unwrap();
    let live = String::from_utf8(output.stderr).unwrap();
    assert!(summary.contains(": passed"));
    assert!(summary.contains("Command build/check: passed"));
    assert!(!summary.contains("human-live"));
    assert!(live.contains("human-live"));
    assert!(live.contains("setup-live"));
}

#[test]
fn promotion_completion_requires_matching_explicit_decisions_after_commands_pass() {
    let fixture = Fixture::shell(
        "printf commands-passed; printf '{\"status\":\"passed\",\"total\":1,\"passed\":1}' > status.json",
    )
    .with_result();
    let manifest = fixture.root.path().join("app.yaml");
    let yaml = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        yaml.replace(
            "    checks:",
            "    require: [approval.local]
    promotion:
      criteria:
        - id: harness
          kind: harness
          check: check
          expected_tests: 1
          minimum_passed: 1
    checks:",
        ),
    )
    .unwrap();
    let help = fixture.command().args(["run", "--help"]).output().unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("BEFORE execution"));
    assert!(help.contains("AFTER commands pass"));
    assert!(help.contains("cancellable Git preflight"));
    assert!(help.contains("Neither gate is auto-approved"));
    assert!(help.contains("report-harness still records external counts explicitly"));
    fixture.register();
    let running = fixture.start(RUN, true);
    let deadline = Instant::now() + Duration::from_secs(10);
    let run_id = loop {
        let runs = fixture.json(&["runs", "demo"]);
        if let Some(run) = runs.as_array().unwrap().first() {
            break run["id"].as_str().unwrap().to_owned();
        }
        assert!(Instant::now() < deadline, "run did not start");
        thread::sleep(Duration::from_millis(25));
    };
    let id = run_id.as_str();
    let waiting = fixture.wait_status(id, "waiting_approval");
    assert_eq!(waiting["commands"][0]["status"], "pending");
    assert!(!fixture.root.path().join("source/status.json").exists());
    let approved = fixture
        .command()
        .args(["approve-stage", id, "--stage", "build", "--approve"])
        .output()
        .unwrap();
    assert!(approved.status.success());
    assert!(approved.stderr.is_empty());
    let approved = String::from_utf8(approved.stdout).unwrap();
    assert!(approved.contains("Local approval recorded; waiting for the runner."));
    assert!(!approved.contains("To approve completion"));
    assert!(!approved.contains("To approve commands"));
    let event = running.wait_output("commands-passed");
    assert_eq!(event["runId"], id);
    let waiting = fixture.wait_status(id, "waiting_approval");
    assert_eq!(waiting["commands"][0]["status"], "passed");
    assert_eq!(waiting["stages"][0]["finishedAt"], Value::Null);
    assert_eq!(fixture.json(&["harness-results"]), json!([]));
    let status = fixture.command().args(["run-status", id]).output().unwrap();
    assert!(status.status.success());
    let status = String::from_utf8(status.stdout).unwrap();
    assert!(status.contains("To approve completion: review promotion-status"));
    assert!(status.contains("approve-promotion"));
    assert!(!status.contains("pipes approve-stage"));

    for release_ref in ["other", "v1"] {
        fixture.json(&[
            "report-harness",
            "demo",
            "--stage",
            "build",
            "--release-ref",
            release_ref,
            "--check",
            "check",
            "--total",
            "1",
            "--passed",
            "1",
        ]);
        let assessment = fixture.json(&[
            "promotion-status",
            "demo",
            "--stage",
            "build",
            "--release-ref",
            release_ref,
        ]);
        assert_eq!(assessment["status"], "ready");
        thread::sleep(Duration::from_millis(100));
        assert_eq!(
            fixture.json(&["run-status", id])["status"],
            "waiting_approval"
        );
        let decision = fixture.json(&[
            "approve-promotion",
            "demo",
            "--stage",
            "build",
            "--release-ref",
            release_ref,
            "--approve",
        ]);
        assert_eq!(decision["assessment"]["releaseRef"], release_ref);
        if release_ref == "other" {
            thread::sleep(Duration::from_millis(100));
            assert_eq!(
                fixture.json(&["run-status", id])["status"],
                "waiting_approval"
            );
        }
    }
    let finished = final_run(&running.finish(), "passed");
    assert_eq!(finished["commands"][0]["status"], "passed");
    assert_eq!(
        fixture.json(&["harness-results"]).as_array().unwrap().len(),
        2
    );
}

#[test]
fn clean_git_preflight_failure_is_a_final_failed_run_without_manifest_execution() {
    let fixture = Fixture::shell("touch execution-marker");
    let source = fixture.root.path().join("source");
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(&source)
            .status()
            .unwrap()
            .success()
    );
    fs::write(source.join("dirty-file"), "untracked").unwrap();
    let manifest = fixture.root.path().join("app.yaml");
    let yaml = fs::read_to_string(&manifest)
        .unwrap()
        .replace("require_clean: false", "require_clean: true");
    fs::write(
        &manifest,
        format!("{yaml}prepare:\n  - {{id: setup, exec: [touch, prepare-marker]}}\n"),
    )
    .unwrap();
    fixture.register();
    let finished = final_run(&fixture.start(RUN, true).finish(), "failed");
    let commands = finished["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 2);
    assert!(
        commands
            .iter()
            .all(|command| command["startedAt"].is_null() && command["exitCode"].is_null())
    );
    assert!(!source.join("execution-marker").exists());
    assert!(!source.join("prepare-marker").exists());
    let id = finished["id"].as_str().unwrap();
    assert_eq!(fixture.json(&["run-status", id]), finished);
    let summaries = fixture.json(&["runs", "demo"]);
    assert_eq!(summaries[0]["status"], "failed");
    assert_eq!(summaries[0]["totalCommands"], 2);
    assert_eq!(summaries[0]["passedCommands"], 0);
}
