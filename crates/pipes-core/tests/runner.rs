#![cfg(unix)]

use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use pipes_core::{
    ApprovePromotion, PipelineRun, PipelineRunner, PromotionContext, RecordHarnessResult,
    RegisteredApp, Registry, RunLogEntry, RunStatus, StartPipelineRun,
};
use serde_json::json;
use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    registry: Registry,
    app: RegisteredApp,
    manifest: PathBuf,
}

impl Fixture {
    fn new(body: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let manifest = root.path().join("pipes.yaml");
        fs::write(&manifest, format!(
            "api: pipes/v1\napp: runner-test\nsource: {{path: ., snapshot: working-tree, require_clean: false}}\nversion: {{from: VERSION}}\n{body}\n"
        )).unwrap();
        let registry = Registry::open(root.path().join("state")).unwrap();
        let app = registry.register(&manifest, None, false).unwrap();
        Self {
            root,
            registry,
            app,
            manifest,
        }
    }

    fn request(&self) -> StartPipelineRun {
        StartPipelineRun {
            app_id: self.app.manifest.app_id.clone(),
            expected_manifest_hash: self.app.manifest.manifest_hash.clone(),
            expected_updated_at: self.app.updated_at.clone(),
            release_ref: "candidate-1".to_owned(),
            approved: true,
        }
    }

    fn wait(&self, id: &str, predicate: impl Fn(&PipelineRun) -> bool) -> PipelineRun {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let run = self.registry.pipeline_run(id).unwrap();
            if predicate(&run) {
                return run;
            }
            assert!(
                Instant::now() < deadline,
                "run did not reach expected state: {run:?}"
            );
            thread::sleep(Duration::from_millis(15));
        }
    }

    fn finished(&self, id: &str) -> PipelineRun {
        self.wait(id, |run| !run.status.is_active())
    }

    fn logs(&self, id: &str) -> Vec<RunLogEntry> {
        let mut cursor = 0;
        let mut entries = Vec::new();
        loop {
            let page = self.registry.pipeline_run_logs(id, cursor).unwrap();
            assert!(page.next_cursor >= cursor);
            cursor = page.next_cursor;
            entries.extend(page.entries);
            if !page.has_more {
                return entries;
            }
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }
}

fn shell(script: &str) -> String {
    json!(["sh", "-c", script]).to_string()
}

#[test]
fn registration_and_rejected_execution_never_spawn_commands() {
    let fixture = Fixture::new(&format!(
        "stages:\n  - id: test\n    checks:\n      - id: test\n        exec: {}",
        shell("touch executed")
    ));
    let runner = PipelineRunner::new(fixture.registry.clone());
    assert!(!fixture.path("executed").exists());
    let mut request = fixture.request();
    request.approved = false;
    assert_eq!(
        runner.start(&request).unwrap_err().code(),
        "run_approval_required"
    );
    request.approved = true;
    request.expected_manifest_hash = "0".repeat(64);
    assert_eq!(
        runner.start(&request).unwrap_err().code(),
        "registration_changed"
    );
    assert!(!fixture.path("executed").exists());
    assert!(fixture.registry.pipeline_runs(None).unwrap().is_empty());
}

#[test]
fn git_snapshot_intent_is_not_silently_run_in_a_working_tree() {
    let mut fixture = Fixture::new("stages: [{id: test}]");
    let yaml = fs::read_to_string(&fixture.manifest)
        .unwrap()
        .replace("snapshot: working-tree", "snapshot: git-commit");
    fs::write(&fixture.manifest, yaml).unwrap();
    fixture.app = fixture
        .registry
        .register(&fixture.manifest, None, true)
        .unwrap();
    let error = PipelineRunner::new(fixture.registry.clone())
        .start(&fixture.request())
        .unwrap_err();
    assert_eq!(error.code(), "execution");
    assert!(error.to_string().contains("working-tree"));
    assert!(fixture.registry.pipeline_runs(None).unwrap().is_empty());
}

#[test]
fn stdout_and_stderr_are_persisted_live_without_waiting_for_newlines_or_exit() {
    let fixture = Fixture::new(&format!(
        "stages:\n  - id: test\n    checks:\n      - id: stream\n        exec: {}",
        shell(
            "printf 'first fragment'; printf 'diagnostic fragment' >&2; while [ ! -f release ]; do sleep 0.02; done; printf '\\nlast line\\n'"
        )
    ));
    let runner = PipelineRunner::new(fixture.registry.clone());
    let run = runner.start(&fixture.request()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let logs = fixture.logs(&run.id);
        if logs
            .iter()
            .any(|entry| entry.text.contains("first fragment"))
            && logs
                .iter()
                .any(|entry| entry.text.contains("diagnostic fragment"))
        {
            assert!(
                fixture
                    .registry
                    .pipeline_run(&run.id)
                    .unwrap()
                    .status
                    .is_active()
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "partial output was not streamed while process was alive"
        );
        thread::sleep(Duration::from_millis(20));
    }
    fs::write(fixture.path("release"), "").unwrap();
    let finished = fixture.finished(&run.id);
    assert_eq!(finished.status, RunStatus::Passed);
    assert_eq!(finished.commands[0].exit_code, Some(0));
    let logs = fixture.logs(&run.id);
    assert!(
        logs.windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    assert!(logs.iter().any(|entry| entry.text.contains("last line")));
    assert!(
        logs.iter()
            .filter(|entry| entry.command_id.as_deref() == Some("stream"))
            .all(|entry| entry.stage_id.as_deref() == Some("test"))
    );
    runner.shutdown().unwrap();
    let reopened = Registry::open(fixture.registry.data_dir()).unwrap();
    assert_eq!(reopened.pipeline_run(&run.id).unwrap(), finished);
}

#[test]
fn json_file_result_is_authoritative_for_normal_exits_and_receives_an_absolute_output_path() {
    let fixture = Fixture::new(&format!(
        "stages:\n  - id: test\n    checks:\n      - id: custom\n        exec: {}\n        result:\n          type: json-file\n          path: status.json\n          pointer: /job/state\n          success: done\n          failure: broken",
        shell(
            "case \"$PIPES_RESULT_FILE\" in /*) ;; *) exit 90;; esac; printf '{\"job\":{\"state\":\"done\"}}' > \"$PIPES_RESULT_FILE\"; exit 7"
        )
    ));
    let runner = PipelineRunner::new(fixture.registry.clone());
    let run = runner.start(&fixture.request()).unwrap();
    let finished = fixture.finished(&run.id);
    assert_eq!(finished.status, RunStatus::Passed);
    assert_eq!(finished.commands[0].exit_code, Some(7));
}

#[test]
fn unchanged_missing_and_unknown_json_results_cannot_pass_even_after_exit_zero() {
    for (before, script) in [
        (Some("{\"status\":\"passed\"}"), "true"),
        (None, "true"),
        (
            None,
            "printf '{\"status\":\"unknown\"}' > \"$PIPES_RESULT_FILE\"",
        ),
    ] {
        let fixture = Fixture::new(&format!(
            "stages:\n  - id: test\n    checks:\n      - id: custom\n        exec: {}\n        result: {{type: json-file, path: status.json, pointer: /status, success: passed, failure: failed}}",
            shell(script)
        ));
        if let Some(before) = before {
            fs::write(fixture.path("status.json"), before).unwrap();
        }
        let runner = PipelineRunner::new(fixture.registry.clone());
        let run = runner.start(&fixture.request()).unwrap();
        let finished = fixture.finished(&run.id);
        assert_eq!(finished.status, RunStatus::Failed);
        assert!(finished.commands[0].message.is_some());
    }
}

#[test]
fn custom_exit_codes_preparation_cwd_and_environment_are_applied() {
    let fixture = Fixture::new(&format!(
        "prepare:\n  - id: directory\n    exec: {}\nstages:\n  - id: test\n    require: [prepare.directory.passed, checks.passed]\n    checks:\n      - id: custom\n        cwd: created\n        env: {{TEST_VALUE: supplied}}\n        exec: {}\n        result: {{type: exit-code, success_codes: [5]}}",
        shell("mkdir created"),
        shell(
            "test \"$TEST_VALUE\" = supplied || exit 9; test \"$PIPES_STAGE_ID\" = test || exit 8; test \"$PIPES_COMMAND_ID\" = custom || exit 7; printf '%s' \"$PIPES_RELEASE_REF\" > result; exit 5"
        )
    ));
    let runner = PipelineRunner::new(fixture.registry.clone());
    let run = runner.start(&fixture.request()).unwrap();
    let finished = fixture.finished(&run.id);
    assert_eq!(finished.status, RunStatus::Passed);
    assert_eq!(finished.commands[0].stage_id, None);
    assert_eq!(finished.commands[1].exit_code, Some(5));
    assert_eq!(
        fs::read_to_string(fixture.path("created/result")).unwrap(),
        "candidate-1"
    );
}

#[test]
fn nested_parallel_budgets_are_shared_and_sequence_barriers_and_all_parent_joins_hold() {
    let guarded = |file: &str| {
        shell(&format!(
            "mkdir shared-budget || exit 90; sleep 0.08; touch {file}; rmdir shared-budget"
        ))
    };
    let fixture = Fixture::new(&format!(
        r#"
execution: {{max_parallel: 2}}
stages:
  - id: first
    after: []
    checks:
      - id: fast-set
        parallel:
          - id: fast-one
            exec: {}
          - id: fast-two
            exec: {}
      - id: bounded-set
        max_parallel: 1
        parallel:
          - id: nested
            max_parallel: 8
            parallel:
              - id: guarded-one
                exec: {}
              - id: guarded-two
                exec: {}
          - id: guarded-three
            exec: {}
      - id: barrier
        exec: {}
  - id: independent
    after: []
    checks:
      - id: independent-command
        exec: {}
  - id: joined
    after: [first, independent]
    checks:
      - id: joined-command
        exec: {}
"#,
        shell("sleep 0.12; touch fast-one"),
        shell("sleep 0.12; touch fast-two"),
        guarded("one"),
        guarded("two"),
        guarded("three"),
        shell(
            "test -f fast-one && test -f fast-two && test -f one && test -f two && test -f three && touch barrier"
        ),
        shell("sleep 0.15; touch independent"),
        shell("test -f barrier && test -f independent && touch joined")
    ));
    let runner = PipelineRunner::new(fixture.registry.clone());
    let run = runner.start(&fixture.request()).unwrap();
    let finished = fixture.finished(&run.id);
    assert_eq!(finished.status, RunStatus::Passed, "{finished:?}");
    assert!(fixture.path("joined").exists());
    let mut events = Vec::new();
    for command in &finished.commands {
        events.push((command.started_at.clone().unwrap(), 1));
        events.push((command.finished_at.clone().unwrap(), -1));
    }
    events.sort();
    let (mut current, mut peak) = (0, 0);
    for (_, delta) in events {
        current += delta;
        peak = peak.max(current);
    }
    assert_eq!(
        peak, 2,
        "the configured budget must be honored without serializing all work"
    );
    let guarded: Vec<_> = finished
        .commands
        .iter()
        .filter(|command| command.command_id.starts_with("guarded"))
        .collect();
    for pair in guarded.windows(2) {
        assert!(
            pair[0].finished_at <= pair[1].started_at,
            "ancestor cap must cover nested parallel groups"
        );
    }
}

#[test]
fn failure_skips_ordered_successors_and_blocks_joins_but_independent_work_finishes() {
    let fixture = Fixture::new(&format!(
        r#"
stages:
  - id: failed
    after: []
    checks:
      - id: batch
        parallel:
          - id: failure
            exec: {}
          - id: sibling
            exec: {}
      - id: never
        exec: {}
  - id: independent
    after: []
    checks: [{{id: independent-command, exec: {}}}]
  - id: joined
    after: [failed, independent]
    checks: [{{id: joined-command, exec: {}}}]
"#,
        shell("exit 3"),
        shell("sleep 0.05; touch sibling"),
        shell("touch never"),
        shell("touch independent"),
        shell("touch joined")
    ));
    let runner = PipelineRunner::new(fixture.registry.clone());
    let run = runner.start(&fixture.request()).unwrap();
    let finished = fixture.finished(&run.id);
    assert_eq!(finished.status, RunStatus::Failed);
    assert_eq!(finished.stages[0].status, RunStatus::Failed);
    assert_eq!(finished.stages[1].status, RunStatus::Passed);
    assert_eq!(finished.stages[2].status, RunStatus::Blocked);
    assert_eq!(finished.commands[2].status, RunStatus::Skipped);
    assert!(fixture.path("sibling").exists());
    assert!(fixture.path("independent").exists());
    assert!(!fixture.path("never").exists());
    assert!(!fixture.path("joined").exists());
}

#[test]
fn local_approval_is_explicit_per_run_and_history_survives_unregistration() {
    let fixture = Fixture::new(&format!(
        "stages:\n  - id: gated\n    require: [approval.local]\n    checks: [{{id: execute, exec: {}}}]",
        shell("touch approved")
    ));
    let runner = PipelineRunner::new(fixture.registry.clone());
    let run = runner.start(&fixture.request()).unwrap();
    fixture.wait(&run.id, |run| run.status == RunStatus::WaitingApproval);
    assert!(!fixture.path("approved").exists());
    assert_eq!(
        fixture
            .registry
            .approve_pipeline_stage(&run.id, "gated", false)
            .unwrap_err()
            .code(),
        "run_approval_required"
    );
    fixture.registry.unregister("runner-test").unwrap();
    fs::remove_file(&fixture.manifest).unwrap();
    fixture
        .registry
        .approve_pipeline_stage(&run.id, "gated", true)
        .unwrap();
    let finished = fixture.finished(&run.id);
    assert_eq!(finished.status, RunStatus::Passed);
    assert!(finished.stages[0].approved_at.is_some());
    assert!(fixture.path("approved").exists());
    assert_eq!(
        fixture.registry.pipeline_runs(Some("runner-test")).unwrap()[0].id,
        run.id
    );
    assert_eq!(
        fixture
            .registry
            .approve_pipeline_stage(&run.id, "gated", true)
            .unwrap_err()
            .code(),
        "run_conflict"
    );
}

#[test]
fn duplicate_source_execution_is_rejected_across_registry_and_runner_handles() {
    let fixture = Fixture::new(&format!(
        "stages:\n  - id: test\n    checks: [{{id: wait, exec: {}}}]",
        shell("sleep 30")
    ));
    let first = PipelineRunner::new(fixture.registry.clone());
    let second = PipelineRunner::new(Registry::open(fixture.registry.data_dir()).unwrap());
    let run = first.start(&fixture.request()).unwrap();
    assert_eq!(
        second.start(&fixture.request()).unwrap_err().code(),
        "run_conflict"
    );
    assert!(
        fixture
            .registry
            .pipeline_run(&run.id)
            .unwrap()
            .status
            .is_active()
    );
    fixture.registry.cancel_pipeline_run(&run.id).unwrap();
    assert_eq!(fixture.finished(&run.id).status, RunStatus::Cancelled);
    first.shutdown().unwrap();
    let next = second.start(&fixture.request()).unwrap();
    assert_ne!(next.id, run.id);
    second.shutdown().unwrap();
    assert_eq!(fixture.finished(&next.id).status, RunStatus::Interrupted);
}

#[test]
fn timeout_and_cancellation_stop_child_process_groups() {
    for timeout in [true, false] {
        let fixture = Fixture::new(&format!(
            "stages:\n  - id: test\n    checks:\n      - id: long\n        exec: {}\n{}",
            shell("touch started; (sleep 0.7; touch escaped) & wait"),
            if timeout {
                "        timeout: 100ms"
            } else {
                ""
            },
        ));
        let runner = PipelineRunner::new(fixture.registry.clone());
        let run = runner.start(&fixture.request()).unwrap();
        fixture.wait(&run.id, |_| fixture.path("started").exists());
        if !timeout {
            fixture.registry.cancel_pipeline_run(&run.id).unwrap();
        }
        let finished = fixture.finished(&run.id);
        assert_eq!(
            finished.status,
            if timeout {
                RunStatus::Failed
            } else {
                RunStatus::Cancelled
            }
        );
        assert_eq!(
            finished.commands[0].status,
            if timeout {
                RunStatus::TimedOut
            } else {
                RunStatus::Cancelled
            }
        );
        thread::sleep(Duration::from_millis(800));
        assert!(
            !fixture.path("escaped").exists(),
            "a descendant survived process-group cancellation"
        );
    }
}

#[test]
fn shutdown_interrupts_instead_of_fabricating_success_and_releases_source_lock() {
    let fixture = Fixture::new(&format!(
        "stages:\n  - id: test\n    checks: [{{id: wait, exec: {}}}]",
        shell("printf live; sleep 30")
    ));
    let runner = PipelineRunner::new(fixture.registry.clone());
    let run = runner.start(&fixture.request()).unwrap();
    fixture.wait(&run.id, |run| run.commands[0].status == RunStatus::Running);
    runner.shutdown().unwrap();
    let finished = fixture.finished(&run.id);
    assert_eq!(finished.status, RunStatus::Interrupted);
    assert_eq!(finished.commands[0].status, RunStatus::Interrupted);
    assert!(
        fixture
            .logs(&run.id)
            .iter()
            .any(|entry| entry.text.contains("Interrupted"))
    );
}

#[test]
fn a_lost_owner_is_recovered_as_interrupted_without_resuming_commands() {
    let fixture = Fixture::new("stages: [{id: test}]");
    let runner = PipelineRunner::new(fixture.registry.clone());
    let started = runner.start(&fixture.request()).unwrap();
    let mut run = fixture.finished(&started.id);
    runner.shutdown().unwrap();
    run.status = RunStatus::Running;
    run.finished_at = None;
    run.stages[0].status = RunStatus::Running;
    run.stages[0].finished_at = None;
    let database =
        rusqlite::Connection::open(fixture.registry.data_dir().join("registry.sqlite3")).unwrap();
    database
        .execute(
            "UPDATE pipeline_runs SET snapshot=?1,active=1 WHERE id=?2",
            rusqlite::params![serde_json::to_string(&run).unwrap(), run.id],
        )
        .unwrap();
    let recovered = fixture.registry.pipeline_run(&run.id).unwrap();
    assert_eq!(recovered.status, RunStatus::Interrupted);
    assert!(recovered.message.unwrap().contains("not resumed"));
    assert_eq!(
        fixture.registry.pipeline_runs(None).unwrap()[0].status,
        RunStatus::Interrupted
    );
}

#[test]
fn promotion_criteria_must_have_a_current_explicit_approval_before_descendants_run() {
    let fixture = Fixture::new(&format!(
        r#"
stages:
  - id: test
    checks: [{{id: suite, exec: {}}}]
    promotion:
      criteria:
        - id: totals
          kind: harness
          check: suite
          expected_tests: 1
          minimum_passed: 1
  - id: release
    checks: [{{id: deliver, exec: {}}}]
"#,
        shell("exit 0"),
        shell("touch released")
    ));
    let runner = PipelineRunner::new(fixture.registry.clone());
    let run = runner.start(&fixture.request()).unwrap();
    let waiting = fixture.wait(&run.id, |run| run.status == RunStatus::WaitingApproval);
    assert_eq!(waiting.commands[0].status, RunStatus::Passed);
    assert!(!fixture.path("released").exists());
    assert_eq!(
        fixture
            .registry
            .approve_pipeline_stage(&run.id, "test", true)
            .unwrap_err()
            .code(),
        "run_conflict"
    );
    let context = PromotionContext {
        app_id: fixture.app.manifest.app_id.clone(),
        stage_id: "test".to_owned(),
        expected_manifest_hash: fixture.app.manifest.manifest_hash.clone(),
        expected_updated_at: fixture.app.updated_at.clone(),
        release_ref: "candidate-1".to_owned(),
    };
    fixture
        .registry
        .record_harness_result(&RecordHarnessResult {
            app_id: context.app_id.clone(),
            stage_id: context.stage_id.clone(),
            check_id: "suite".to_owned(),
            expected_manifest_hash: context.expected_manifest_hash.clone(),
            expected_updated_at: context.expected_updated_at.clone(),
            release_ref: context.release_ref.clone(),
            total: 1,
            passed: 1,
            notes: None,
        })
        .unwrap();
    let assessment = fixture.registry.assess_promotion(&context).unwrap();
    fixture
        .registry
        .approve_promotion(&ApprovePromotion {
            app_id: context.app_id,
            stage_id: context.stage_id,
            expected_manifest_hash: context.expected_manifest_hash,
            expected_updated_at: context.expected_updated_at,
            release_ref: context.release_ref,
            expected_assessment_hash: assessment.assessment_hash,
            approved: true,
            acknowledge_optional: false,
            notes: None,
        })
        .unwrap();
    assert_eq!(fixture.finished(&run.id).status, RunStatus::Passed);
    assert!(fixture.path("released").exists());
}

#[test]
fn run_logs_are_paginated_without_gaps_and_invalid_controls_are_explicit_errors() {
    let fixture = Fixture::new(&format!(
        "stages:\n  - id: test\n    checks: [{{id: output, exec: {}}}]",
        shell("dd if=/dev/zero bs=4096 count=300 2>/dev/null | tr '\\000' x")
    ));
    let runner = PipelineRunner::new(fixture.registry.clone());
    let run = runner.start(&fixture.request()).unwrap();
    assert_eq!(fixture.finished(&run.id).status, RunStatus::Passed);
    let page = fixture.registry.pipeline_run_logs(&run.id, 0).unwrap();
    assert_eq!(page.entries.len(), 256);
    assert!(page.has_more);
    let logs = fixture.logs(&run.id);
    let stdout: String = logs
        .iter()
        .filter(|entry| entry.stream == pipes_core::LogStream::Stdout)
        .map(|entry| entry.text.as_str())
        .collect();
    assert_eq!(stdout.len(), 4096 * 300);
    assert!(stdout.bytes().all(|byte| byte == b'x'));
    assert_eq!(
        fixture
            .registry
            .pipeline_run("not-an-id")
            .unwrap_err()
            .code(),
        "invalid_run"
    );
    assert_eq!(
        fixture.registry.pipeline_run("999999").unwrap_err().code(),
        "run_not_found"
    );
    assert_eq!(
        fixture
            .registry
            .pipeline_run_logs(&run.id, u64::MAX)
            .unwrap_err()
            .code(),
        "invalid_run"
    );
}

#[test]
fn shared_result_files_are_serialized_and_cannot_reuse_another_checks_success() {
    let fixture = Fixture::new(&format!(
        "execution: {{max_parallel: 2}}\nstages:\n  - id: test\n    checks:\n      - id: together\n        parallel:\n          - id: producer\n            exec: {}\n            result: {{type: json-file, path: shared.json, pointer: /status, success: passed, failure: failed}}\n          - id: non-producer\n            exec: {}\n            result: {{type: json-file, path: ./shared.json, pointer: /status, success: passed, failure: failed}}",
        shell("sleep 0.15; printf '{\"status\":\"passed\"}' > \"$PIPES_RESULT_FILE\""),
        shell("sleep 0.3"),
    ));
    let runner = PipelineRunner::new(fixture.registry.clone());
    let run = runner.start(&fixture.request()).unwrap();
    let finished = fixture.finished(&run.id);
    assert_eq!(finished.status, RunStatus::Failed);
    assert_eq!(finished.commands[0].status, RunStatus::Passed);
    assert_eq!(finished.commands[1].status, RunStatus::Failed);
    assert!(finished.commands[0].finished_at <= finished.commands[1].started_at);
    assert!(
        finished.commands[1]
            .message
            .as_deref()
            .unwrap()
            .contains("stale")
    );
}

#[test]
fn clean_source_preflight_is_recorded_and_runs_before_manifest_commands() {
    for clean in [true, false] {
        let mut fixture = Fixture::new(&format!(
            "stages:\n  - id: test\n    checks: [{{id: command, exec: {}}}]",
            shell("touch executed")
        ));
        let yaml = fs::read_to_string(&fixture.manifest)
            .unwrap()
            .replace("require_clean: false", "require_clean: true");
        fs::write(&fixture.manifest, yaml).unwrap();
        fixture.app = fixture
            .registry
            .register(&fixture.manifest, None, true)
            .unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(fixture.root.path())
                .status()
                .unwrap()
                .success()
        );
        if clean {
            fs::write(fixture.path(".git/info/exclude"), "*\n").unwrap();
        }
        let runner = PipelineRunner::new(fixture.registry.clone());
        let run = runner.start(&fixture.request()).unwrap();
        let finished = fixture.finished(&run.id);
        assert_eq!(
            finished.status,
            if clean {
                RunStatus::Passed
            } else {
                RunStatus::Failed
            }
        );
        assert_eq!(fixture.path("executed").exists(), clean);
        assert!(
            fixture
                .logs(&run.id)
                .iter()
                .any(|entry| entry.text.contains("clean Git source"))
        );
        assert_eq!(
            finished.commands.len(),
            1,
            "preflight must not invent a manifest check"
        );
    }
}
