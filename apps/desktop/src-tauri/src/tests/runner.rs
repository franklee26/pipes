use super::*;
use std::{
    thread,
    time::{Duration, Instant},
};

fn execution_fixture(script: &str, result: bool) -> (TempDir, PathBuf, PathBuf) {
    let (directory, manifest, data_dir) = fixture();
    fs::create_dir(directory.path().join("source")).unwrap();
    let result = if result {
        "        result: {type: json-file, path: status.json, pointer: /status, success: passed, failure: failed}\n"
    } else {
        ""
    };
    fs::write(
        &manifest,
        format!(
            "api: pipes/v1\napp: atlas\nname: Atlas\nsource: {{path: source, snapshot: working-tree, require_clean: false}}\nversion: {{from: VERSION}}\nexecution: {{timeout: 10s}}\nstages:\n  - id: test\n    checks:\n      - id: unit\n        exec: {}\n{result}",
            json!(["sh", "-c", script])
        ),
    )
    .unwrap();
    (directory, manifest, data_dir)
}

fn start_request(registered: &Value) -> Value {
    json!({
        "appId": "atlas",
        "expectedManifestHash": registered["manifest"]["manifestHash"],
        "expectedUpdatedAt": registered["updatedAt"],
        "releaseRef": "v1",
        "approved": true
    })
}

fn start(window: &tauri::WebviewWindow<MockRuntime>, registered: &Value) -> Value {
    invoke(
        window,
        "start_pipeline_run",
        json!({ "request": start_request(registered) }),
    )
    .expect("start an approved pipeline")
}

fn wait_status(window: &tauri::WebviewWindow<MockRuntime>, run_id: &str, status: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let run = invoke(window, "get_pipeline_run", json!({ "runId": run_id }))
            .expect("read pipeline run");
        if run["status"] == status {
            return run;
        }
        assert!(Instant::now() < deadline, "waiting for {status}: {run}");
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_output(window: &tauri::WebviewWindow<MockRuntime>, run_id: &str, text: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let page = invoke(
            window,
            "pipeline_run_logs",
            json!({ "runId": run_id, "after": 0 }),
        )
        .expect("read live logs");
        if page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["text"].as_str().unwrap().contains(text))
        {
            return page;
        }
        assert!(Instant::now() < deadline, "waiting for {text:?}: {page}");
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn native_start_returns_active_run_and_incremental_partial_logs_before_json_result() {
    let (directory, manifest, data_dir) = execution_fixture(
        "printf partial; printf warning >&2; while [ ! -f continue ]; do sleep 0.02; done; printf '\\nfinished'; printf '{\"status\":\"passed\"}' > status.json",
        true,
    );
    let app = test_app(data_dir.clone());
    assert!(!data_dir.exists());
    assert!(
        app.state::<Arc<RunnerManager>>()
            .runner
            .lock()
            .unwrap()
            .is_none()
    );
    let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
        .build()
        .unwrap();
    let registered = register_reviewed_manifest(&window, &manifest, false);
    let run = start(&window, &registered);
    let id = run["id"].as_str().unwrap();
    assert!(matches!(
        run["status"].as_str(),
        Some("pending" | "running")
    ));
    assert!(!directory.path().join("source/status.json").exists());
    let page = wait_output(&window, id, "partial");
    let entry = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "partial")
        .expect("partial output without a newline");
    assert_eq!(entry["stageId"], "test");
    assert_eq!(entry["commandId"], "unit");
    assert_eq!(entry["stream"], "stdout");
    let cursor = page["nextCursor"].as_u64().unwrap();
    let later = invoke(
        &window,
        "pipeline_run_logs",
        json!({ "runId": id, "after": cursor }),
    )
    .unwrap();
    assert!(
        later["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["sequence"].as_u64().unwrap() > cursor)
    );
    let summaries = invoke(&window, "list_pipeline_runs", json!({ "appId": "atlas" })).unwrap();
    assert_eq!(summaries[0]["id"], id);
    assert!(summaries[0].get("manifest").is_none());
    invoke(&window, "unregister_app", json!({ "appId": "atlas" })).unwrap();
    assert_eq!(invoke(&window, "list_apps", json!({})).unwrap(), json!([]));
    assert_eq!(
        invoke(&window, "get_pipeline_run", json!({ "runId": id })).unwrap()["status"],
        "running"
    );
    fs::write(directory.path().join("source/continue"), "").unwrap();
    let finished = wait_status(&window, id, "passed");
    assert_eq!(finished["manifest"], registered["manifest"]);
    assert_eq!(finished["commands"][0]["status"], "passed");
    assert_eq!(finished["commands"][0]["exitCode"], 0);
    let page = invoke(
        &window,
        "pipeline_run_logs",
        json!({ "runId": id, "after": 0 }),
    )
    .unwrap();
    assert!(!page["hasMore"].as_bool().unwrap());
    let text = |stream: &str| {
        page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["stream"] == stream)
            .map(|entry| entry["text"].as_str().unwrap())
            .collect::<String>()
    };
    assert_eq!(text("stdout"), "partial\nfinished");
    assert_eq!(text("stderr"), "warning");
    let cursor = page["nextCursor"].as_u64().unwrap();
    assert_eq!(
        invoke(
            &window,
            "pipeline_run_logs",
            json!({ "runId": id, "after": cursor })
        )
        .unwrap(),
        json!({ "entries": [], "nextCursor": cursor, "hasMore": false })
    );
    fs::remove_file(&manifest).unwrap();
    assert_eq!(
        invoke(&window, "get_pipeline_run", json!({ "runId": id })).unwrap(),
        finished
    );
    assert_eq!(
        invoke(&window, "list_pipeline_runs", json!({ "appId": "atlas" })).unwrap()[0]["status"],
        "passed"
    );
    assert_eq!(
        invoke(&window, "list_pipeline_runs", json!({ "appId": "other" })).unwrap(),
        json!([])
    );
    assert_eq!(
        Registry::open(data_dir)
            .unwrap()
            .pipeline_run(id)
            .unwrap()
            .status,
        pipes_core::RunStatus::Passed
    );
}

#[test]
fn native_start_requires_explicit_consent_current_tokens_and_only_typed_requests() {
    let (directory, manifest, data_dir) = execution_fixture("touch execution-marker", false);
    let app = test_app(data_dir);
    let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
        .build()
        .unwrap();
    let registered = register_reviewed_manifest(&window, &manifest, false);
    let request = start_request(&registered);
    for (field, value, code) in [
        ("approved", json!(false), "run_approval_required"),
        (
            "expectedManifestHash",
            json!("0".repeat(64)),
            "registration_changed",
        ),
        (
            "expectedUpdatedAt",
            json!("2000-01-01T00:00:00Z"),
            "registration_changed",
        ),
        ("releaseRef", json!(""), "invalid_run"),
    ] {
        let mut changed = request.clone();
        changed[field] = value;
        assert_request_error(&window, "start_pipeline_run", changed, code);
    }
    for field in [
        "approved",
        "expectedManifestHash",
        "expectedUpdatedAt",
        "releaseRef",
    ] {
        let mut missing = request.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(invoke(&window, "start_pipeline_run", json!({ "request": missing })).is_err());
    }
    let mut injected = request;
    injected["exec"] = json!(["touch", "unreviewed-marker"]);
    assert!(
        invoke(
            &window,
            "start_pipeline_run",
            json!({ "request": injected })
        )
        .is_err()
    );
    assert!(
        invoke(
            &window,
            "run_command",
            json!({ "exec": ["touch", "generic-marker"] })
        )
        .is_err()
    );
    assert_eq!(
        invoke(&window, "list_pipeline_runs", json!({ "appId": null })).unwrap(),
        json!([])
    );
    for name in ["execution-marker", "unreviewed-marker", "generic-marker"] {
        assert!(!directory.path().join("source").join(name).exists());
    }
    let yaml = fs::read_to_string(&manifest).unwrap();
    fs::write(&manifest, yaml.replace("name: Atlas", "name: Updated")).unwrap();
    register_reviewed_manifest(&window, &manifest, true);
    assert_request_error(
        &window,
        "start_pipeline_run",
        start_request(&registered),
        "registration_changed",
    );
    assert!(!directory.path().join("source/execution-marker").exists());
}

#[test]
fn native_history_and_controls_report_stable_unknown_id_and_cursor_errors() {
    let (_directory, _manifest, data_dir) = fixture();
    let app = test_app(data_dir);
    let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
        .build()
        .unwrap();
    assert_eq!(
        invoke(&window, "list_pipeline_runs", json!({ "appId": null })).unwrap(),
        json!([])
    );
    for (command, args) in [
        ("get_pipeline_run", json!({ "runId": "99999" })),
        ("pipeline_run_logs", json!({ "runId": "99999", "after": 0 })),
        ("cancel_pipeline_run", json!({ "runId": "99999" })),
        (
            "approve_pipeline_stage",
            json!({ "runId": "99999", "stageId": "test", "approved": true }),
        ),
    ] {
        let error = invoke(&window, command, args).unwrap_err();
        assert_eq!(error["code"], "run_not_found", "{command}: {error}");
        assert!(error["message"].as_str().unwrap().len() > 10);
    }
    let error = invoke(
        &window,
        "pipeline_run_logs",
        json!({ "runId": "99999", "after": 9_007_199_254_740_992u64 }),
    )
    .unwrap_err();
    assert_eq!(error["code"], "invalid_run");
    assert!(
        invoke(
            &window,
            "pipeline_run_logs",
            json!({ "runId": "99999", "after": -1 })
        )
        .is_err()
    );
    assert!(
        app.state::<Arc<RunnerManager>>()
            .runner
            .lock()
            .unwrap()
            .is_none()
    );
}

#[test]
fn native_stage_gate_waits_for_explicit_approval_from_another_adapter() {
    let (directory, manifest, data_dir) = execution_fixture("touch gated-marker", false);
    let yaml = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        yaml.replace(
            "  - id: test",
            "  - id: test\n    require: [approval.local]",
        ),
    )
    .unwrap();
    let owner = test_app(data_dir.clone());
    let owner_window = tauri::WebviewWindowBuilder::new(owner.handle(), "main", Default::default())
        .build()
        .unwrap();
    let registered = register_reviewed_manifest(&owner_window, &manifest, false);
    let run = start(&owner_window, &registered);
    let id = run["id"].as_str().unwrap();
    let waiting = wait_status(&owner_window, id, "waiting_approval");
    assert_eq!(waiting["stages"][0]["approvedAt"], Value::Null);
    assert!(!directory.path().join("source/gated-marker").exists());

    let controls = test_app(data_dir);
    let controls_window =
        tauri::WebviewWindowBuilder::new(controls.handle(), "main", Default::default())
            .build()
            .unwrap();
    let denied = invoke(
        &controls_window,
        "approve_pipeline_stage",
        json!({ "runId": id, "stageId": "test", "approved": false }),
    )
    .unwrap_err();
    assert_eq!(denied["code"], "run_approval_required");
    let unknown = invoke(
        &controls_window,
        "approve_pipeline_stage",
        json!({ "runId": id, "stageId": "missing", "approved": true }),
    )
    .unwrap_err();
    assert_eq!(unknown["code"], "stage_not_found");
    assert_eq!(
        invoke(&controls_window, "get_pipeline_run", json!({ "runId": id })).unwrap()["status"],
        "waiting_approval"
    );
    assert!(!directory.path().join("source/gated-marker").exists());
    invoke(
        &controls_window,
        "approve_pipeline_stage",
        json!({ "runId": id, "stageId": "test", "approved": true }),
    )
    .unwrap();
    let finished = wait_status(&owner_window, id, "passed");
    assert!(finished["stages"][0]["approvedAt"].is_string());
    assert!(directory.path().join("source/gated-marker").is_file());
    assert!(
        controls
            .state::<Arc<RunnerManager>>()
            .runner
            .lock()
            .unwrap()
            .is_none()
    );
    let already_finished = invoke(
        &controls_window,
        "approve_pipeline_stage",
        json!({ "runId": id, "stageId": "test", "approved": true }),
    )
    .unwrap_err();
    assert_eq!(already_finished["code"], "run_conflict");
}

#[test]
fn native_failed_commands_and_failed_json_verdicts_are_terminal_failures() {
    for (script, result) in [
        ("printf failure >&2; exit 7", false),
        ("printf '{\"status\":\"failed\"}' > status.json", true),
    ] {
        let (_directory, manifest, data_dir) = execution_fixture(script, result);
        let app = test_app(data_dir);
        let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
            .build()
            .unwrap();
        let registered = register_reviewed_manifest(&window, &manifest, false);
        let run = start(&window, &registered);
        let id = run["id"].as_str().unwrap();
        let finished = wait_status(&window, id, "failed");
        assert_eq!(finished["commands"][0]["status"], "failed");
        assert!(finished["commands"][0]["message"].is_string());
        assert_eq!(
            invoke(&window, "list_pipeline_runs", json!({ "appId": null })).unwrap()[0]["status"],
            "failed"
        );
    }
}

#[test]
fn native_cancellation_stops_active_children_and_is_persisted() {
    let (directory, manifest, data_dir) = execution_fixture(
        "printf ready; (sleep 3; printf leaked > orphan-marker) & wait",
        false,
    );
    let app = test_app(data_dir.clone());
    let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
        .build()
        .unwrap();
    let registered = register_reviewed_manifest(&window, &manifest, false);
    let run = start(&window, &registered);
    let id = run["id"].as_str().unwrap();
    wait_output(&window, id, "ready");
    let requested = invoke(&window, "cancel_pipeline_run", json!({ "runId": id })).unwrap();
    assert_eq!(requested["id"], id);
    let cancelled = wait_status(&window, id, "cancelled");
    assert_eq!(cancelled["commands"][0]["status"], "cancelled");
    assert!(cancelled["finishedAt"].is_string());
    assert_eq!(
        Registry::open(data_dir)
            .unwrap()
            .pipeline_run(id)
            .unwrap()
            .status,
        pipes_core::RunStatus::Cancelled
    );
    thread::sleep(Duration::from_millis(3100));
    assert!(!directory.path().join("source/orphan-marker").exists());
}

#[test]
fn dropping_native_app_interrupts_and_drops_its_owned_runner() {
    let (directory, manifest, data_dir) = execution_fixture(
        "printf ready; (sleep 3; printf leaked > orphan-marker) & wait",
        false,
    );
    let app = test_app(data_dir.clone());
    let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
        .build()
        .unwrap();
    let registered = register_reviewed_manifest(&window, &manifest, false);
    let run = start(&window, &registered);
    let id = run["id"].as_str().unwrap();
    wait_output(&window, id, "ready");
    let manager = Arc::clone(app.state::<Arc<RunnerManager>>().inner());
    drop(window);
    drop(app);
    assert!(manager.shutting_down.load(Ordering::Acquire));
    assert!(manager.runner.lock().unwrap().is_none());
    let persisted = Registry::open(data_dir).unwrap().pipeline_run(id).unwrap();
    assert_eq!(persisted.status, pipes_core::RunStatus::Interrupted);
    assert_eq!(
        persisted.commands[0].status,
        pipes_core::RunStatus::Interrupted
    );
    thread::sleep(Duration::from_millis(3100));
    assert!(!directory.path().join("source/orphan-marker").exists());
}

#[test]
fn lazy_native_runner_retries_real_location_errors_without_creating_a_fallback() {
    let (_directory, manifest, data_dir) = execution_fixture("printf ok", false);
    fs::write(&data_dir, "not a registry directory").unwrap();
    let app = test_app(data_dir.clone());
    assert!(
        app.state::<Arc<RunnerManager>>()
            .runner
            .lock()
            .unwrap()
            .is_none()
    );
    let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
        .build()
        .unwrap();
    let request = json!({
        "appId": "atlas",
        "expectedManifestHash": "0".repeat(64),
        "expectedUpdatedAt": "2000-01-01T00:00:00Z",
        "releaseRef": "v1",
        "approved": true
    });
    let error = invoke(&window, "start_pipeline_run", json!({ "request": request })).unwrap_err();
    assert!(error["code"].is_string());
    assert!(data_dir.is_file());
    assert!(
        app.state::<Arc<RunnerManager>>()
            .runner
            .lock()
            .unwrap()
            .is_none()
    );
    fs::remove_file(&data_dir).unwrap();
    let registered = register_reviewed_manifest(&window, &manifest, false);
    let run = start(&window, &registered);
    wait_status(&window, run["id"].as_str().unwrap(), "passed");
    app.state::<Arc<RunnerManager>>().shutdown().unwrap();
    assert!(
        app.state::<Arc<RunnerManager>>()
            .runner
            .lock()
            .unwrap()
            .is_none()
    );
    assert_request_error(
        &window,
        "start_pipeline_run",
        start_request(&registered),
        "execution",
    );
}

#[test]
fn native_local_and_promotion_gates_apply_before_and_after_command_execution() {
    let (directory, manifest, data_dir) = execution_fixture(
        "printf commands-passed; printf ran >> execution-count; printf '{\"status\":\"passed\",\"total\":1,\"passed\":1}' > status.json",
        true,
    );
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
          check: unit
          expected_tests: 1
          minimum_passed: 1
    checks:",
        ),
    )
    .unwrap();
    let app = test_app(data_dir);
    let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
        .build()
        .unwrap();
    let registered = register_reviewed_manifest(&window, &manifest, false);
    let run = start(&window, &registered);
    let id = run["id"].as_str().unwrap();
    let waiting = wait_status(&window, id, "waiting_approval");
    assert_eq!(waiting["commands"][0]["status"], "pending");
    assert_eq!(waiting["stages"][0]["approvedAt"], Value::Null);
    assert!(!directory.path().join("source/status.json").exists());
    invoke(
        &window,
        "approve_pipeline_stage",
        json!({ "runId": id, "stageId": "test", "approved": true }),
    )
    .unwrap();
    wait_output(&window, id, "commands-passed");
    let waiting = wait_status(&window, id, "waiting_approval");
    assert_eq!(waiting["commands"][0]["status"], "passed");
    assert!(waiting["stages"][0]["approvedAt"].is_string());
    assert_eq!(waiting["stages"][0]["finishedAt"], Value::Null);
    assert_eq!(
        invoke(&window, "list_harness_results", json!({ "appId": "atlas" })).unwrap(),
        json!([])
    );
    let wrong_gate = invoke(
        &window,
        "approve_pipeline_stage",
        json!({ "runId": id, "stageId": "test", "approved": true }),
    )
    .unwrap_err();
    assert_eq!(wrong_gate["code"], "run_conflict");
    for release_ref in ["other", "v1"] {
        let context = promotion_context(&registered, "test", release_ref);
        invoke(
            &window,
            "record_harness_result",
            json!({ "request": harness_request(&context, 1, 1) }),
        )
        .unwrap();
        let assessment =
            invoke(&window, "assess_promotion", json!({ "request": context })).unwrap();
        assert_eq!(assessment["status"], "ready");
        thread::sleep(Duration::from_millis(100));
        assert_eq!(
            invoke(&window, "get_pipeline_run", json!({ "runId": id })).unwrap()["status"],
            "waiting_approval"
        );
        let decision = invoke(
            &window,
            "approve_promotion",
            json!({ "request": promotion_approval_request(&context, &assessment) }),
        )
        .unwrap();
        assert_eq!(decision["assessment"]["releaseRef"], release_ref);
        if release_ref == "other" {
            thread::sleep(Duration::from_millis(100));
            assert_eq!(
                invoke(&window, "get_pipeline_run", json!({ "runId": id })).unwrap()["status"],
                "waiting_approval"
            );
        }
    }
    let finished = wait_status(&window, id, "passed");
    assert_eq!(finished["commands"][0]["status"], "passed");
    assert_eq!(
        fs::read_to_string(directory.path().join("source/execution-count")).unwrap(),
        "ran"
    );
    assert_eq!(
        invoke(&window, "list_harness_results", json!({ "appId": "atlas" }))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn native_clean_git_preflight_failure_is_accepted_then_persisted_without_execution() {
    let (directory, manifest, data_dir) = execution_fixture("touch execution-marker", false);
    let source = directory.path().join("source");
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&source)
            .status()
            .unwrap()
            .success()
    );
    fs::write(source.join("dirty-file"), "untracked").unwrap();
    let yaml = fs::read_to_string(&manifest)
        .unwrap()
        .replace("require_clean: false", "require_clean: true");
    fs::write(
        &manifest,
        format!("{yaml}prepare:\n  - {{id: setup, exec: [touch, prepare-marker]}}\n"),
    )
    .unwrap();
    let app = test_app(data_dir);
    let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
        .build()
        .unwrap();
    let registered = register_reviewed_manifest(&window, &manifest, false);
    let accepted = start(&window, &registered);
    assert!(matches!(
        accepted["status"].as_str(),
        Some("pending" | "running")
    ));
    let id = accepted["id"].as_str().unwrap();
    let failed = wait_status(&window, id, "failed");
    let commands = failed["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 2);
    assert!(
        commands
            .iter()
            .all(|command| command["startedAt"].is_null() && command["exitCode"].is_null())
    );
    assert!(!source.join("execution-marker").exists());
    assert!(!source.join("prepare-marker").exists());
    let summaries = invoke(&window, "list_pipeline_runs", json!({ "appId": "atlas" })).unwrap();
    assert_eq!(summaries[0]["status"], "failed");
    assert_eq!(summaries[0]["totalCommands"], 2);
    assert_eq!(summaries[0]["passedCommands"], 0);
}
