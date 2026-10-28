use pipes_core::{
    ApprovePromotion, ConsumerDeployment, HarnessResult, ManifestPreview, PipelineRun,
    PipelineRunSummary, PipelineRunner, PromotionApproval, PromotionAssessment, PromotionContext,
    RecordConsumerDeployment, RecordHarnessResult, RegisteredApp, Registry, RunLogPage,
    StartPipelineRun,
};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{Manager, Runtime, State};

#[derive(Clone, Default)]
struct RegistryLocation {
    data_dir_override: Option<PathBuf>,
}

impl RegistryLocation {
    fn data_dir(&self) -> pipes_core::Result<PathBuf> {
        match &self.data_dir_override {
            Some(path) => Ok(path.clone()),
            None => pipes_core::default_data_dir(),
        }
    }

    fn open(&self) -> pipes_core::Result<Registry> {
        Registry::open(self.data_dir()?)
    }
}

#[derive(Default)]
struct RunnerManager {
    runner: Mutex<Option<PipelineRunner>>,
    shutting_down: AtomicBool,
}

impl RunnerManager {
    fn start(
        &self,
        location: &RegistryLocation,
        request: &StartPipelineRun,
    ) -> pipes_core::Result<PipelineRun> {
        let mut runner = self
            .runner
            .lock()
            .map_err(|_| pipes_core::Error::Execution {
                message: "The desktop runner lock was poisoned; restart Pipes.".to_owned(),
            })?;
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(pipes_core::Error::Execution {
                message: "Pipes is shutting down; new pipeline runs cannot start.".to_owned(),
            });
        }
        if runner.is_none() {
            *runner = Some(PipelineRunner::new(location.open()?));
        }
        runner
            .as_ref()
            .expect("runner initialized above")
            .start(request)
    }

    fn shutdown(&self) -> pipes_core::Result<()> {
        self.shutting_down.store(true, Ordering::Release);
        // Even a poisoned start must not leave child processes behind on exit.
        let mut owner = self
            .runner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(runner) = owner.take() {
            runner.shutdown()?;
        }
        Ok(())
    }
}

impl Drop for RunnerManager {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            eprintln!("Pipes could not finish runner shutdown: {error}");
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandError {
    code: String,
    message: String,
}

impl From<pipes_core::Error> for CommandError {
    fn from(error: pipes_core::Error) -> Self {
        Self {
            code: error.code().to_owned(),
            message: error.to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegistryInfo {
    data_dir: String,
    database_path: String,
    app_version: &'static str,
    os: &'static str,
    arch: &'static str,
}

async fn blocking<T, F>(operation: F) -> Result<T, CommandError>
where
    T: Send + 'static,
    F: FnOnce() -> pipes_core::Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|error| CommandError {
            code: "internal".to_owned(),
            message: format!("Local registry task failed: {error}"),
        })?
        .map_err(CommandError::from)
}

#[tauri::command]
async fn registry_info(
    location: State<'_, RegistryLocation>,
) -> Result<RegistryInfo, CommandError> {
    let location = location.inner().clone();
    blocking(move || {
        let data_dir = location.data_dir()?;
        Ok(RegistryInfo {
            data_dir: data_dir.display().to_string(),
            database_path: data_dir.join("registry.sqlite3").display().to_string(),
            app_version: env!("CARGO_PKG_VERSION"),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        })
    })
    .await
}

#[tauri::command]
async fn list_apps(
    location: State<'_, RegistryLocation>,
) -> Result<Vec<RegisteredApp>, CommandError> {
    let location = location.inner().clone();
    blocking(move || location.open()?.list()).await
}

#[tauri::command]
async fn list_pipeline_runs(
    location: State<'_, RegistryLocation>,
    app_id: Option<String>,
) -> Result<Vec<PipelineRunSummary>, CommandError> {
    let location = location.inner().clone();
    blocking(move || location.open()?.pipeline_runs(app_id.as_deref())).await
}

#[tauri::command]
async fn start_pipeline_run(
    location: State<'_, RegistryLocation>,
    runner: State<'_, Arc<RunnerManager>>,
    request: StartPipelineRun,
) -> Result<PipelineRun, CommandError> {
    let location = location.inner().clone();
    let runner = Arc::clone(runner.inner());
    blocking(move || runner.start(&location, &request)).await
}

#[tauri::command]
async fn get_pipeline_run(
    location: State<'_, RegistryLocation>,
    run_id: String,
) -> Result<PipelineRun, CommandError> {
    let location = location.inner().clone();
    blocking(move || location.open()?.pipeline_run(&run_id)).await
}

#[tauri::command]
async fn pipeline_run_logs(
    location: State<'_, RegistryLocation>,
    run_id: String,
    after: u64,
) -> Result<RunLogPage, CommandError> {
    let location = location.inner().clone();
    blocking(move || location.open()?.pipeline_run_logs(&run_id, after)).await
}

#[tauri::command]
async fn cancel_pipeline_run(
    location: State<'_, RegistryLocation>,
    run_id: String,
) -> Result<PipelineRun, CommandError> {
    let location = location.inner().clone();
    blocking(move || location.open()?.cancel_pipeline_run(&run_id)).await
}

#[tauri::command]
async fn approve_pipeline_stage(
    location: State<'_, RegistryLocation>,
    run_id: String,
    stage_id: String,
    approved: bool,
) -> Result<PipelineRun, CommandError> {
    let location = location.inner().clone();
    blocking(move || {
        location
            .open()?
            .approve_pipeline_stage(&run_id, &stage_id, approved)
    })
    .await
}

#[tauri::command]
async fn preview_manifest(path: String) -> Result<ManifestPreview, CommandError> {
    blocking(move || pipes_core::preview_manifest(path)).await
}

#[tauri::command]
async fn register_manifest(
    location: State<'_, RegistryLocation>,
    path: String,
    expected_hash: String,
    replace: bool,
) -> Result<RegisteredApp, CommandError> {
    let location = location.inner().clone();
    blocking(move || {
        location
            .open()?
            .register(path, Some(&expected_hash), replace)
    })
    .await
}

#[tauri::command]
async fn list_consumer_deployments(
    location: State<'_, RegistryLocation>,
    app_id: Option<String>,
) -> Result<Vec<ConsumerDeployment>, CommandError> {
    let location = location.inner().clone();
    blocking(move || location.open()?.consumer_deployments(app_id.as_deref())).await
}

#[tauri::command]
async fn record_consumer_deployment(
    location: State<'_, RegistryLocation>,
    request: RecordConsumerDeployment,
) -> Result<ConsumerDeployment, CommandError> {
    let location = location.inner().clone();
    blocking(move || location.open()?.record_consumer_deployment(&request)).await
}

#[tauri::command]
async fn list_harness_results(
    location: State<'_, RegistryLocation>,
    app_id: Option<String>,
) -> Result<Vec<HarnessResult>, CommandError> {
    let location = location.inner().clone();
    blocking(move || location.open()?.harness_results(app_id.as_deref())).await
}

#[tauri::command]
async fn record_harness_result(
    location: State<'_, RegistryLocation>,
    request: RecordHarnessResult,
) -> Result<HarnessResult, CommandError> {
    let location = location.inner().clone();
    blocking(move || location.open()?.record_harness_result(&request)).await
}

#[tauri::command]
async fn assess_promotion(
    location: State<'_, RegistryLocation>,
    request: PromotionContext,
) -> Result<PromotionAssessment, CommandError> {
    let location = location.inner().clone();
    blocking(move || location.open()?.assess_promotion(&request)).await
}

#[tauri::command]
async fn approve_promotion(
    location: State<'_, RegistryLocation>,
    request: ApprovePromotion,
) -> Result<PromotionApproval, CommandError> {
    let location = location.inner().clone();
    blocking(move || location.open()?.approve_promotion(&request)).await
}

#[tauri::command]
async fn list_promotion_approvals(
    location: State<'_, RegistryLocation>,
    app_id: Option<String>,
) -> Result<Vec<PromotionApproval>, CommandError> {
    let location = location.inner().clone();
    blocking(move || location.open()?.promotion_approvals(app_id.as_deref())).await
}

#[tauri::command]
async fn unregister_app(
    location: State<'_, RegistryLocation>,
    app_id: String,
) -> Result<(), CommandError> {
    let location = location.inner().clone();
    blocking(move || location.open()?.unregister(&app_id)).await
}

fn with_registry_commands<R: Runtime>(
    builder: tauri::Builder<R>,
    location: RegistryLocation,
) -> tauri::Builder<R> {
    builder
        .manage(location)
        .manage(Arc::new(RunnerManager::default()))
        .invoke_handler(tauri::generate_handler![
            registry_info,
            list_apps,
            list_pipeline_runs,
            start_pipeline_run,
            get_pipeline_run,
            pipeline_run_logs,
            cancel_pipeline_run,
            approve_pipeline_stage,
            preview_manifest,
            register_manifest,
            list_consumer_deployments,
            record_consumer_deployment,
            list_harness_results,
            record_harness_result,
            assess_promotion,
            approve_promotion,
            list_promotion_approvals,
            unregister_app
        ])
}

pub fn run() {
    let app = match with_registry_commands(tauri::Builder::default(), RegistryLocation::default())
        .plugin(tauri_plugin_dialog::init())
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(error) => {
            eprintln!("Pipes could not start: {error}");
            std::process::exit(1);
        }
    };
    app.run(|app, event| {
        if matches!(
            event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) && let Err(error) = app.state::<Arc<RunnerManager>>().shutdown()
        {
            eprintln!("Pipes could not finish runner shutdown: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::{fs, path::Path};
    use tauri::test::{MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets};
    use tauri::webview::InvokeRequest;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, PathBuf, PathBuf) {
        let directory = tempfile::Builder::new()
            .prefix(".pipes-native-test-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("test directory");
        let manifest = directory.path().join("pipes.yaml");
        let data_dir = directory.path().join("registry");
        fs::write(
            &manifest,
            "api: pipes/v1\napp: atlas\nname: Atlas\nsource:\n  path: .\nversion:\n  from: VERSION\nstages:\n  - id: test\n    checks:\n      - id: unit\n        exec: [touch, execution-marker]\n    require: [checks.passed]\n",
        )
        .expect("fixture manifest");
        (directory, manifest, data_dir)
    }

    fn consumer_fixture() -> (TempDir, PathBuf, PathBuf) {
        let (directory, manifest, data_dir) = fixture();
        let target = json!(directory.path().join("consumer destination"));
        let exec = json!(["touch", directory.path().join("execution-marker")]);
        let original = fs::read_to_string(&manifest)
            .expect("fixture content")
            .replace("[touch, execution-marker]", &exec.to_string());
        fs::write(
            &manifest,
            format!(
                "{original}    consumers:\n      - id: files\n        name: Local files\n        target: {target}\n        approval: manual\n"
            ),
        )
        .expect("consumer manifest");
        fs::write(
            directory.path().join("artifact.txt"),
            "not copied or deployed",
        )
        .expect("source artifact");
        (directory, manifest, data_dir)
    }

    fn promotion_fixture() -> (TempDir, PathBuf, PathBuf) {
        let (directory, manifest, data_dir) = consumer_fixture();
        let original = fs::read_to_string(&manifest).expect("consumer manifest");
        fs::write(
            &manifest,
            format!(
                "{original}      - id: personal
        name: Personal device
        target: display-only:personal-device
        approval: manual
    promotion:
      criteria:
        - id: smoke-tests
          kind: harness
          check: unit
          expected_tests: 50
          minimum_passed: 45
        - id: device-required
          kind: deployment
          consumer: files
          requirement: required
        - id: personal-optional
          kind: deployment
          consumer: personal
          requirement: optional
  - id: release
    consumers:
      - id: files
        name: Production
        target: display-only:production
        approval: manual
"
            ),
        )
        .expect("promotion manifest");
        (directory, manifest, data_dir)
    }

    fn assert_no_execution_or_copy(directory: &Path) {
        assert!(!directory.join("execution-marker").exists());
        assert!(!directory.join("consumer destination").exists());
        assert!(!directory.join("replacement destination").exists());
        assert_eq!(
            fs::read_to_string(directory.join("artifact.txt")).expect("unchanged artifact"),
            "not copied or deployed"
        );
    }

    struct TestApp(tauri::App<MockRuntime>);

    impl std::ops::Deref for TestApp {
        type Target = tauri::App<MockRuntime>;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl Drop for TestApp {
        fn drop(&mut self) {
            // MockRuntime retains app handles and does not deliver RunEvent::Exit.
            if let Err(error) = self.state::<Arc<RunnerManager>>().shutdown() {
                eprintln!("Test runner shutdown failed: {error}");
            }
        }
    }

    fn test_app(data_dir: PathBuf) -> TestApp {
        TestApp(
            with_registry_commands(
                mock_builder(),
                RegistryLocation {
                    data_dir_override: Some(data_dir),
                },
            )
            .build(mock_context(noop_assets()))
            .expect("mock application"),
        )
    }

    fn request(
        window: &tauri::WebviewWindow<MockRuntime>,
        command: &str,
        args: Value,
    ) -> InvokeRequest {
        InvokeRequest {
            cmd: command.to_owned(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: window.url().expect("local webview URL"),
            body: tauri::ipc::InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_owned(),
        }
    }

    fn invoke(
        window: &tauri::WebviewWindow<MockRuntime>,
        command: &str,
        args: Value,
    ) -> Result<Value, Value> {
        get_ipc_response(window, request(window, command, args))
            .map(|body| body.deserialize::<Value>().expect("JSON response"))
    }

    fn register_reviewed_manifest(
        window: &tauri::WebviewWindow<MockRuntime>,
        manifest: &Path,
        replace: bool,
    ) -> Value {
        let preview =
            invoke(window, "preview_manifest", json!({ "path": manifest })).expect("preview");
        let registered = invoke(
            window,
            "register_manifest",
            json!({
                "path": manifest,
                "expectedHash": preview["manifestHash"],
                "replace": replace
            }),
        )
        .expect("register reviewed manifest");
        assert_eq!(registered["manifest"], preview);
        registered
    }

    fn promotion_context(registered: &Value, stage_id: &str, release_ref: &str) -> Value {
        json!({
            "appId": "atlas",
            "stageId": stage_id,
            "expectedManifestHash": registered["manifest"]["manifestHash"],
            "expectedUpdatedAt": registered["updatedAt"],
            "releaseRef": release_ref
        })
    }

    fn harness_request(context: &Value, total: u32, passed: u32) -> Value {
        let mut request = context.clone();
        request["checkId"] = json!("unit");
        request["total"] = json!(total);
        request["passed"] = json!(passed);
        request["notes"] = Value::Null;
        request
    }

    fn deployment_request(context: &Value, consumer_id: &str) -> Value {
        let mut request = context.clone();
        request["consumerId"] = json!(consumer_id);
        request["approved"] = json!(true);
        request["notes"] = Value::Null;
        request
    }

    fn promotion_approval_request(context: &Value, assessment: &Value) -> Value {
        let mut request = context.clone();
        request["expectedAssessmentHash"] = assessment["assessmentHash"].clone();
        request["approved"] = json!(true);
        request["acknowledgeOptional"] = json!(false);
        request["notes"] = Value::Null;
        request
    }

    fn assert_request_error(
        window: &tauri::WebviewWindow<MockRuntime>,
        command: &str,
        request: Value,
        code: &str,
    ) {
        let failure = invoke(window, command, json!({ "request": request }))
            .expect_err("invalid request must fail");
        assert_eq!(failure["code"], code, "{failure}");
        assert!(
            !failure["message"]
                .as_str()
                .expect("error message")
                .is_empty()
        );
    }

    #[test]
    fn real_ipc_round_trip_shares_the_core_registry_without_execution() {
        let (directory, manifest, data_dir) = fixture();
        let app = test_app(data_dir.clone());
        let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
            .build()
            .expect("mock webview");

        let info = invoke(&window, "registry_info", json!({})).expect("registry info");
        assert_eq!(
            info["databasePath"],
            data_dir
                .join("registry.sqlite3")
                .to_str()
                .expect("UTF-8 path")
        );
        assert!(!data_dir.exists());

        let preview = invoke(&window, "preview_manifest", json!({ "path": manifest }))
            .expect("manifest preview");
        assert_eq!(preview["appId"], "atlas");
        assert_eq!(preview["versionFrom"], "VERSION");
        assert_eq!(preview["execution"]["maxParallel"], "auto");
        assert_eq!(preview["stages"][0]["after"], json!([]));
        assert_eq!(preview["stages"][0]["consumers"], json!([]));
        assert_eq!(
            preview["stages"][0]["checks"][0]["exec"],
            json!(["touch", "execution-marker"])
        );
        assert_eq!(
            preview["stages"][0]["checkPlan"],
            json!([{
                "kind": "command",
                "command": preview["stages"][0]["checks"][0]
            }])
        );
        assert!(!data_dir.exists());

        let registered = invoke(
            &window,
            "register_manifest",
            json!({
                "path": manifest,
                "expectedHash": preview["manifestHash"],
                "replace": false
            }),
        )
        .expect("registration through IPC");
        assert_eq!(registered["manifest"], preview);
        assert!(registered["registeredAt"].is_string());

        let records = invoke(&window, "list_apps", json!({})).expect("registered applications");
        assert_eq!(records, json!([registered]));
        let core = Registry::open(data_dir).expect("independent core connection");
        assert_eq!(
            core.get("atlas")
                .expect("persisted registration")
                .manifest
                .app_id,
            "atlas"
        );
        assert!(!directory.path().join("execution-marker").exists());

        let result = invoke(&window, "unregister_app", json!({ "appId": "atlas" }))
            .expect("unregister through camelCase IPC");
        assert!(result.is_null());
        assert!(core.list().expect("remaining registrations").is_empty());
        assert!(manifest.exists());
        assert!(directory.path().exists());
    }

    #[test]
    fn branched_example_preserves_forks_and_join_parents_through_ipc() {
        let (_directory, manifest, data_dir) = fixture();
        let yaml =
            include_str!("../../../../examples/forks.yaml").replace("  path: ..", "  path: .");
        fs::write(&manifest, yaml).expect("branched example");
        let app = test_app(data_dir);
        let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
            .build()
            .expect("mock webview");
        let preview = invoke(&window, "preview_manifest", json!({ "path": manifest }))
            .expect("branched preview");
        assert_eq!(preview["appId"], "pipes-forks");
        assert_eq!(preview["stages"][0]["after"], json!([]));
        assert_eq!(preview["stages"][1]["after"], json!(["test"]));
        assert_eq!(preview["stages"][2]["after"], json!(["test"]));
        assert_eq!(preview["stages"][3]["after"], json!(["test"]));
        assert_eq!(
            preview["stages"][4]["after"],
            json!(["beta-linux", "beta-macos"])
        );
        assert_eq!(preview["stages"][5]["after"], json!(["gamma"]));
        let registered = invoke(
            &window,
            "register_manifest",
            json!({ "path": manifest, "expectedHash": preview["manifestHash"], "replace": false }),
        )
        .expect("branched registration");
        assert_eq!(registered["manifest"], preview);
        assert_eq!(
            invoke(&window, "list_apps", json!({})).expect("saved graph"),
            json!([registered])
        );
    }

    #[test]
    fn composed_check_example_preserves_barriers_and_caps_through_ipc() {
        let (_directory, manifest, data_dir) = fixture();
        let yaml = include_str!("../../../../examples/check-sequences.yaml")
            .replace("  path: ..", "  path: .");
        fs::write(&manifest, yaml).expect("composed check example");
        let app = test_app(data_dir);
        let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
            .build()
            .expect("mock webview");
        let preview = invoke(&window, "preview_manifest", json!({ "path": manifest }))
            .expect("composed plan preview");
        assert_eq!(preview["execution"]["maxParallel"], "auto");
        let beta = &preview["stages"][1];
        assert_eq!(beta["id"], "beta");
        assert_eq!(
            beta["checks"].as_array().expect("flattened checks").len(),
            5
        );
        assert_eq!(beta["checkPlan"].as_array().expect("ordered plan").len(), 2);
        assert_eq!(beta["checkPlan"][0]["kind"], "parallel");
        assert_eq!(beta["checkPlan"][0]["id"], "quick-tests");
        assert!(beta["checkPlan"][0]["maxParallel"].is_null());
        assert_eq!(beta["checkPlan"][1]["id"], "acceptance");
        assert_eq!(beta["checkPlan"][1]["maxParallel"], 2);
        assert_eq!(beta["checkPlan"][1]["steps"][1]["kind"], "sequence");
        assert_eq!(
            beta["checkPlan"][1]["steps"][1]["steps"][0]["command"]["id"],
            "frontend-build"
        );
        assert_eq!(
            beta["checkPlan"][1]["steps"][1]["steps"][1]["command"]["id"],
            "example-validation"
        );
        let registered = invoke(
            &window,
            "register_manifest",
            json!({ "path": manifest, "expectedHash": preview["manifestHash"], "replace": false }),
        )
        .expect("composed registration");
        assert_eq!(registered["manifest"], preview);
        fs::remove_file(&manifest).expect("remove original fixture");
        assert_eq!(
            invoke(&window, "list_apps", json!({})).expect("saved composed plan"),
            json!([registered])
        );
    }

    #[test]
    fn ipc_rejects_manifest_changes_since_the_reviewed_hash() {
        let (_directory, manifest, data_dir) = fixture();
        let app = test_app(data_dir.clone());
        let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
            .build()
            .expect("mock webview");
        let preview =
            invoke(&window, "preview_manifest", json!({ "path": manifest })).expect("preview");
        let original = fs::read_to_string(&manifest).expect("fixture content");
        fs::write(&manifest, original.replace("name: Atlas", "name: Edited"))
            .expect("change fixture");
        let failure = invoke(
            &window,
            "register_manifest",
            json!({ "path": manifest, "expectedHash": preview["manifestHash"], "replace": false }),
        )
        .expect_err("stale review must fail");
        assert_eq!(failure["code"], "manifest_changed");
        assert!(failure["message"].is_string());
        assert!(
            Registry::open(data_dir)
                .expect("registry")
                .list()
                .expect("list")
                .is_empty()
        );
    }

    #[test]
    fn consumer_deployment_ipc_requires_approval_and_retains_immutable_history() {
        let (directory, manifest, data_dir) = consumer_fixture();
        let app = test_app(data_dir.clone());
        let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
            .build()
            .expect("mock webview");
        let registered = register_reviewed_manifest(&window, &manifest, false);
        assert_eq!(
            registered["manifest"]["stages"][0]["consumers"],
            json!([{
                "id": "files",
                "name": "Local files",
                "target": directory.path().join("consumer destination"),
                "approval": "manual"
            }])
        );
        let approved = json!({
            "appId": "atlas",
            "stageId": "test",
            "consumerId": "files",
            "expectedManifestHash": registered["manifest"]["manifestHash"],
            "expectedUpdatedAt": registered["updatedAt"],
            "releaseRef": "refs/tags/v1",
            "notes": "Verified outside Pipes",
            "approved": true
        });
        let mut missing_approval = approved.clone();
        missing_approval
            .as_object_mut()
            .expect("request object")
            .remove("approved");
        let failure = invoke(
            &window,
            "record_consumer_deployment",
            json!({ "request": missing_approval }),
        )
        .expect_err("missing approval must fail deserialization");
        assert!(
            failure
                .as_str()
                .expect("IPC argument error")
                .contains("approved")
        );
        let mut unknown_field = approved.clone();
        unknown_field["target"] = json!("caller-selected-target");
        let failure = invoke(
            &window,
            "record_consumer_deployment",
            json!({ "request": unknown_field }),
        )
        .expect_err("a caller cannot override the manifest target");
        assert!(
            failure
                .as_str()
                .expect("IPC argument error")
                .contains("target")
        );
        for (field, value, code) in [
            ("approved", json!(false), "approval_required"),
            ("releaseRef", json!(""), "invalid_deployment"),
            ("releaseRef", json!(" \t "), "invalid_deployment"),
            ("consumerId", json!("missing"), "consumer_not_found"),
        ] {
            let mut invalid = approved.clone();
            invalid[field] = value;
            let failure = invoke(
                &window,
                "record_consumer_deployment",
                json!({ "request": invalid }),
            )
            .expect_err("invalid recording must fail");
            assert_eq!(failure["code"], code);
            assert!(failure["message"].is_string());
        }
        assert_eq!(
            invoke(
                &window,
                "list_consumer_deployments",
                json!({ "appId": null })
            )
            .expect("empty history"),
            json!([])
        );
        let first = invoke(
            &window,
            "record_consumer_deployment",
            json!({ "request": approved }),
        )
        .expect("record manually approved deployment");
        assert!(
            !first["id"]
                .as_str()
                .expect("opaque deployment ID")
                .is_empty()
        );
        assert!(
            !first["recordedAt"]
                .as_str()
                .expect("recorded timestamp")
                .is_empty()
        );
        assert_eq!(
            first,
            json!({
                "id": first["id"],
                "appId": "atlas",
                "appName": "Atlas",
                "stageId": "test",
                "consumer": registered["manifest"]["stages"][0]["consumers"][0],
                "releaseRef": "refs/tags/v1",
                "notes": "Verified outside Pipes",
                "manifestHash": registered["manifest"]["manifestHash"],
                "manifestPath": registered["manifest"]["manifestPath"],
                "sourcePath": registered["manifest"]["sourcePath"],
                "registrationUpdatedAt": registered["updatedAt"],
                "recordedAt": first["recordedAt"]
            })
        );
        for app_id in [json!(null), json!("atlas")] {
            assert_eq!(
                invoke(
                    &window,
                    "list_consumer_deployments",
                    json!({ "appId": app_id })
                )
                .expect("recorded history"),
                json!([first])
            );
        }
        assert_eq!(
            invoke(
                &window,
                "list_consumer_deployments",
                json!({ "appId": "unknown" })
            )
            .expect("unknown app has empty history"),
            json!([])
        );
        assert_eq!(
            serde_json::to_value(
                Registry::open(data_dir)
                    .expect("independent registry connection")
                    .consumer_deployments(Some("atlas"))
                    .expect("shared history")
            )
            .expect("serialized history"),
            json!([first])
        );

        let yaml = fs::read_to_string(&manifest).expect("consumer manifest");
        fs::write(
            &manifest,
            yaml.replace("name: Atlas", "name: Updated Atlas")
                .replace("consumer destination", "replacement destination"),
        )
        .expect("retarget consumer");
        let replacement = register_reviewed_manifest(&window, &manifest, true);
        let mut current = approved.clone();
        current["expectedManifestHash"] = replacement["manifest"]["manifestHash"].clone();
        current["expectedUpdatedAt"] = replacement["updatedAt"].clone();
        current["releaseRef"] = json!("refs/tags/v2");
        current
            .as_object_mut()
            .expect("request object")
            .remove("notes");
        for field in ["expectedManifestHash", "expectedUpdatedAt"] {
            assert_ne!(current[field], approved[field]);
            let mut stale = current.clone();
            stale[field] = approved[field].clone();
            let failure = invoke(
                &window,
                "record_consumer_deployment",
                json!({ "request": stale }),
            )
            .expect_err("a stale review cannot record a deployment");
            assert_eq!(failure["code"], "registration_changed");
            assert!(failure["message"].is_string());
        }
        let second = invoke(
            &window,
            "record_consumer_deployment",
            json!({ "request": current }),
        )
        .expect("record reviewed replacement");
        assert_eq!(second["appName"], "Updated Atlas");
        assert_eq!(second["notes"], Value::Null);
        assert_eq!(
            second["consumer"]["target"],
            json!(directory.path().join("replacement destination"))
        );
        let history = json!([second, first]);
        assert_eq!(
            invoke(
                &window,
                "list_consumer_deployments",
                json!({ "appId": "atlas" })
            )
            .expect("newest first with original immutable destination"),
            history
        );
        invoke(&window, "unregister_app", json!({ "appId": "atlas" }))
            .expect("unregister without deleting history");
        assert_eq!(
            invoke(&window, "list_apps", json!({})).expect("no registrations"),
            json!([])
        );
        for app_id in [json!(null), json!("atlas")] {
            assert_eq!(
                invoke(
                    &window,
                    "list_consumer_deployments",
                    json!({ "appId": app_id })
                )
                .expect("history retained after unregistration"),
                history
            );
        }
        assert!(manifest.is_file());
        assert!(!directory.path().join("execution-marker").exists());
        assert!(!directory.path().join("consumer destination").exists());
        assert!(!directory.path().join("replacement destination").exists());
        assert_eq!(
            fs::read_to_string(directory.path().join("artifact.txt")).expect("unchanged artifact"),
            "not copied or deployed"
        );
    }

    #[test]
    fn promotion_ipc_round_trip_persists_camel_case_assessments_and_immutable_skip_history() {
        let (directory, manifest, data_dir) = promotion_fixture();
        let app = test_app(data_dir.clone());
        let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
            .build()
            .expect("mock webview");
        let registered = register_reviewed_manifest(&window, &manifest, false);
        let preview = &registered["manifest"];
        let criteria = json!([
            {
                "kind": "harness",
                "id": "smoke-tests",
                "checkId": "unit",
                "expectedTests": 50,
                "minimumPassed": 45
            },
            {
                "kind": "deployment",
                "id": "device-required",
                "consumerId": "files",
                "requirement": "required"
            },
            {
                "kind": "deployment",
                "id": "personal-optional",
                "consumerId": "personal",
                "requirement": "optional"
            }
        ]);
        assert_eq!(preview["stages"][0]["promotionCriteria"], criteria);
        assert_eq!(preview["stages"][1]["promotionCriteria"], json!([]));
        let context = promotion_context(&registered, "test", "v1");
        let initial = invoke(&window, "assess_promotion", json!({ "request": context }))
            .expect("blocked assessment is a successful IPC result");
        assert_eq!(initial["status"], "blocked");
        assert_eq!(initial["criteria"][0]["observed"], Value::Null);
        assert_eq!(initial["criteria"][1]["status"], "blocked");
        assert_eq!(initial["criteria"][2]["status"], "warning");
        for command in ["list_harness_results", "list_promotion_approvals"] {
            assert_eq!(
                invoke(&window, command, json!({ "appId": null })).expect("empty history"),
                json!([])
            );
        }

        let mut report = harness_request(&context, 50, 45);
        report["notes"] = json!("Reported by external CI");
        let reported = invoke(
            &window,
            "record_harness_result",
            json!({ "request": report }),
        )
        .expect("report without any approval field");
        assert!(!reported["id"].as_str().expect("result ID").is_empty());
        assert!(
            !reported["recordedAt"]
                .as_str()
                .expect("recorded timestamp")
                .is_empty()
        );
        assert_eq!(
            reported,
            json!({
                "id": reported["id"],
                "appId": "atlas",
                "appName": "Atlas",
                "stageId": "test",
                "checkId": "unit",
                "releaseRef": "v1",
                "total": 50,
                "passed": 45,
                "notes": "Reported by external CI",
                "manifestHash": preview["manifestHash"],
                "manifestPath": preview["manifestPath"],
                "sourcePath": preview["sourcePath"],
                "registrationUpdatedAt": registered["updatedAt"],
                "recordedAt": reported["recordedAt"]
            })
        );
        let blocked = invoke(&window, "assess_promotion", json!({ "request": context }))
            .expect("required deployment remains missing");
        assert_request_error(
            &window,
            "approve_promotion",
            promotion_approval_request(&context, &blocked),
            "promotion_blocked",
        );
        let required = invoke(
            &window,
            "record_consumer_deployment",
            json!({ "request": deployment_request(&context, "files") }),
        )
        .expect("manual required deployment");
        let ready = invoke(&window, "assess_promotion", json!({ "request": context }))
            .expect("optional deployment is a warning");
        assert!(
            !ready["assessmentHash"]
                .as_str()
                .expect("assessment hash")
                .is_empty()
        );
        assert_eq!(
            ready,
            json!({
                "appId": "atlas",
                "appName": "Atlas",
                "stageId": "test",
                "releaseRef": "v1",
                "manifestHash": preview["manifestHash"],
                "manifestPath": preview["manifestPath"],
                "sourcePath": preview["sourcePath"],
                "registrationUpdatedAt": registered["updatedAt"],
                "status": "ready_with_warnings",
                "criteria": [
                    {
                        "criterion": criteria[0],
                        "status": "satisfied",
                        "message": ready["criteria"][0]["message"],
                        "evidenceId": reported["id"],
                        "observed": {"total": 50, "passed": 45},
                        "consumer": null
                    },
                    {
                        "criterion": criteria[1],
                        "status": "satisfied",
                        "message": ready["criteria"][1]["message"],
                        "evidenceId": required["id"],
                        "observed": null,
                        "consumer": preview["stages"][0]["consumers"][0]
                    },
                    {
                        "criterion": criteria[2],
                        "status": "warning",
                        "message": ready["criteria"][2]["message"],
                        "evidenceId": null,
                        "observed": null,
                        "consumer": preview["stages"][0]["consumers"][1]
                    }
                ],
                "stageRequirements": ["checks.passed"],
                "assessmentHash": ready["assessmentHash"]
            })
        );
        for criterion in ready["criteria"].as_array().expect("criteria") {
            assert!(
                !criterion["message"]
                    .as_str()
                    .expect("criterion reason")
                    .is_empty()
            );
        }
        let mut approval_request = promotion_approval_request(&context, &ready);
        assert_request_error(
            &window,
            "approve_promotion",
            approval_request.clone(),
            "optional_steps_unacknowledged",
        );
        approval_request["acknowledgeOptional"] = json!(true);
        approval_request["notes"] = json!("Personal device deliberately skipped");
        let approved = invoke(
            &window,
            "approve_promotion",
            json!({ "request": approval_request }),
        )
        .expect("explicitly acknowledge optional warning");
        assert!(!approved["id"].as_str().expect("approval ID").is_empty());
        assert!(
            !approved["recordedAt"]
                .as_str()
                .expect("approval timestamp")
                .is_empty()
        );
        assert_eq!(
            approved,
            json!({
                "id": approved["id"],
                "assessment": ready,
                "skippedOptional": ["personal-optional"],
                "notes": "Personal device deliberately skipped",
                "recordedAt": approved["recordedAt"]
            })
        );

        let failed = invoke(
            &window,
            "record_harness_result",
            json!({ "request": harness_request(&context, 50, 44) }),
        )
        .expect("record later failure");
        invoke(
            &window,
            "record_consumer_deployment",
            json!({ "request": deployment_request(&context, "personal") }),
        )
        .expect("later optional deployment");
        let blocked = invoke(&window, "assess_promotion", json!({ "request": context }))
            .expect("new failure blocks only current readiness");
        assert_eq!(blocked["status"], "blocked");
        assert_eq!(
            invoke(
                &window,
                "list_promotion_approvals",
                json!({ "appId": "atlas" })
            )
            .expect("original decision is immutable"),
            json!([approved])
        );
        let recovered = invoke(
            &window,
            "record_harness_result",
            json!({ "request": harness_request(&context, 50, 45) }),
        )
        .expect("later passing evidence");
        let current = invoke(&window, "assess_promotion", json!({ "request": context }))
            .expect("all criteria now satisfied");
        assert_eq!(current["status"], "ready");
        let second = invoke(
            &window,
            "approve_promotion",
            json!({ "request": promotion_approval_request(&context, &current) }),
        )
        .expect("fully satisfied policy needs no optional acknowledgement");
        assert_eq!(second["assessment"], current);
        assert_eq!(second["skippedOptional"], json!([]));
        assert_eq!(second["notes"], Value::Null);
        let reports = json!([recovered, failed, reported]);
        let approvals = json!([second, approved]);
        let core = Registry::open(data_dir).expect("independent connection");
        assert_eq!(
            serde_json::to_value(
                core.harness_results(Some("atlas"))
                    .expect("persisted results")
            )
            .expect("serialized results"),
            reports
        );
        assert_eq!(
            serde_json::to_value(
                core.promotion_approvals(Some("atlas"))
                    .expect("persisted approvals")
            )
            .expect("serialized approvals"),
            approvals
        );
        invoke(&window, "unregister_app", json!({ "appId": "atlas" }))
            .expect("archive registration only");
        for (command, history) in [
            ("list_harness_results", &reports),
            ("list_promotion_approvals", &approvals),
        ] {
            for app_id in [json!(null), json!("atlas")] {
                assert_eq!(
                    &invoke(&window, command, json!({ "appId": app_id }))
                        .expect("immutable archived history"),
                    history
                );
            }
            assert_eq!(
                invoke(&window, command, json!({ "appId": "unknown" }))
                    .expect("empty app-specific history"),
                json!([])
            );
        }
        assert_eq!(
            fs::read_to_string(&manifest).expect("unchanged manifest"),
            preview["manifestYaml"].as_str().expect("snapshot YAML")
        );
        assert_no_execution_or_copy(directory.path());
    }

    #[test]
    fn promotion_ipc_uses_exact_counts_latest_reports_and_candidate_scoped_evidence() {
        let (directory, manifest, data_dir) = promotion_fixture();
        let app = test_app(data_dir);
        let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
            .build()
            .expect("mock webview");
        let registered = register_reviewed_manifest(&window, &manifest, false);
        let context = promotion_context(&registered, "test", "v1");
        let other = promotion_context(&registered, "test", "other");
        invoke(
            &window,
            "record_harness_result",
            json!({ "request": harness_request(&other, 50, 45) }),
        )
        .expect("other candidate result");
        invoke(
            &window,
            "record_consumer_deployment",
            json!({ "request": deployment_request(&other, "files") }),
        )
        .expect("other candidate deployment");
        invoke(
            &window,
            "record_consumer_deployment",
            json!({
                "request": deployment_request(
                    &promotion_context(&registered, "release", "v1"),
                    "files"
                )
            }),
        )
        .expect("other stage deployment");
        let empty = invoke(&window, "assess_promotion", json!({ "request": context }))
            .expect("no matching evidence");
        assert_eq!(empty["status"], "blocked");
        assert_eq!(empty["criteria"][0]["evidenceId"], Value::Null);
        assert_eq!(empty["criteria"][1]["evidenceId"], Value::Null);
        invoke(
            &window,
            "record_consumer_deployment",
            json!({ "request": deployment_request(&context, "files") }),
        )
        .expect("matching required deployment");
        for (total, passed, expected_status) in [
            (50, 44, "blocked"),
            (49, 45, "blocked"),
            (51, 50, "blocked"),
            (0, 0, "blocked"),
            (50, 45, "ready_with_warnings"),
            (50, 50, "ready_with_warnings"),
            (50, 0, "blocked"),
        ] {
            let result = invoke(
                &window,
                "record_harness_result",
                json!({ "request": harness_request(&context, total, passed) }),
            )
            .expect("valid externally reported counts");
            let assessment = invoke(&window, "assess_promotion", json!({ "request": context }))
                .expect("current assessment");
            assert_eq!(assessment["status"], expected_status, "{total}/{passed}");
            assert_eq!(assessment["criteria"][0]["evidenceId"], result["id"]);
            assert_eq!(
                assessment["criteria"][0]["observed"],
                json!({ "total": total, "passed": passed })
            );
        }
        assert_no_execution_or_copy(directory.path());
    }

    #[test]
    fn promotion_ipc_rejects_stale_assessment_and_registration_reviews() {
        let (directory, manifest, data_dir) = promotion_fixture();
        let app = test_app(data_dir);
        let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
            .build()
            .expect("mock webview");
        let registered = register_reviewed_manifest(&window, &manifest, false);
        let context = promotion_context(&registered, "test", "v1");
        invoke(
            &window,
            "record_harness_result",
            json!({ "request": harness_request(&context, 50, 45) }),
        )
        .expect("matching counts");
        invoke(
            &window,
            "record_consumer_deployment",
            json!({ "request": deployment_request(&context, "files") }),
        )
        .expect("required deployment");
        let previous = invoke(&window, "assess_promotion", json!({ "request": context }))
            .expect("review before optional deployment");
        invoke(
            &window,
            "record_consumer_deployment",
            json!({ "request": deployment_request(&context, "personal") }),
        )
        .expect("new evidence");
        let current = invoke(&window, "assess_promotion", json!({ "request": context }))
            .expect("review after optional deployment");
        assert_ne!(previous["assessmentHash"], current["assessmentHash"]);
        let mut stale_assessment = promotion_approval_request(&context, &previous);
        stale_assessment["acknowledgeOptional"] = json!(true);
        assert_request_error(
            &window,
            "approve_promotion",
            stale_assessment,
            "assessment_changed",
        );
        assert_eq!(
            invoke(
                &window,
                "list_promotion_approvals",
                json!({ "appId": null })
            )
            .expect("no stale approvals recorded"),
            json!([])
        );
        fs::write(
            &manifest,
            registered["manifest"]["manifestYaml"]
                .as_str()
                .expect("original YAML")
                .replace("name: Atlas", "name: Updated Atlas"),
        )
        .expect("replace registration");
        let replacement = register_reviewed_manifest(&window, &manifest, true);
        let fresh_context = promotion_context(&replacement, "test", "v1");
        let fresh = invoke(
            &window,
            "assess_promotion",
            json!({ "request": fresh_context }),
        )
        .expect("new registration excludes old evidence");
        assert_eq!(fresh["status"], "blocked");
        for criterion in fresh["criteria"].as_array().expect("criteria") {
            assert_eq!(criterion["evidenceId"], Value::Null);
        }
        for (command, valid) in [
            (
                "record_harness_result",
                harness_request(&fresh_context, 50, 45),
            ),
            ("assess_promotion", fresh_context.clone()),
            (
                "approve_promotion",
                promotion_approval_request(&fresh_context, &fresh),
            ),
        ] {
            for field in ["expectedManifestHash", "expectedUpdatedAt"] {
                assert_ne!(fresh_context[field], context[field]);
                let mut stale = valid.clone();
                stale[field] = context[field].clone();
                assert_request_error(&window, command, stale, "registration_changed");
            }
        }
        assert_no_execution_or_copy(directory.path());
    }

    #[test]
    fn promotion_ipc_validates_flat_requests_counts_and_explicit_approval() {
        let (directory, manifest, data_dir) = promotion_fixture();
        let app = test_app(data_dir);
        let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
            .build()
            .expect("mock webview");
        let registered = register_reviewed_manifest(&window, &manifest, false);
        let context = promotion_context(&registered, "test", "v1");
        let report = harness_request(&context, 50, 45);
        for (field, value, code) in [
            ("passed", json!(51), "invalid_harness_result"),
            ("checkId", json!("missing"), "check_not_found"),
            ("stageId", json!("missing"), "stage_not_found"),
            ("appId", json!("missing"), "not_found"),
        ] {
            let mut invalid = report.clone();
            invalid[field] = value;
            assert_request_error(&window, "record_harness_result", invalid, code);
        }
        for (field, value) in [
            ("total", json!(-1)),
            ("passed", json!(-1)),
            ("total", json!(4_294_967_296_u64)),
            ("passed", json!(4_294_967_296_u64)),
            ("total", json!(50.5)),
            ("passed", json!("45")),
        ] {
            let mut invalid = report.clone();
            invalid[field] = value;
            let failure = invoke(
                &window,
                "record_harness_result",
                json!({ "request": invalid }),
            )
            .expect_err("counts must deserialize as u32");
            assert!(
                failure
                    .as_str()
                    .expect("IPC deserialization error")
                    .contains("invalid")
            );
        }
        let assessment = invoke(&window, "assess_promotion", json!({ "request": context }))
            .expect("blocked assessment");
        let approved = promotion_approval_request(&context, &assessment);
        let mut unapproved = approved.clone();
        unapproved["approved"] = json!(false);
        assert_request_error(
            &window,
            "approve_promotion",
            unapproved,
            "approval_required",
        );
        for field in ["approved", "expectedAssessmentHash", "expectedUpdatedAt"] {
            let mut missing = approved.clone();
            missing
                .as_object_mut()
                .expect("request object")
                .remove(field);
            let failure = invoke(&window, "approve_promotion", json!({ "request": missing }))
                .expect_err("required approval fields cannot be omitted");
            assert!(
                failure
                    .as_str()
                    .expect("IPC argument error")
                    .contains(field)
            );
        }
        let mut wrong_case = context.clone();
        wrong_case
            .as_object_mut()
            .expect("context")
            .remove("stageId");
        wrong_case["stage_id"] = json!("test");
        assert!(
            invoke(
                &window,
                "assess_promotion",
                json!({ "request": wrong_case })
            )
            .expect_err("snake_case cannot replace camelCase")
            .is_string()
        );
        for (field, value, code) in [
            ("releaseRef", json!(" \t "), "invalid_promotion"),
            ("stageId", json!("missing"), "stage_not_found"),
            ("appId", json!("missing"), "not_found"),
        ] {
            let mut invalid = context.clone();
            invalid[field] = value;
            assert_request_error(&window, "assess_promotion", invalid, code);
        }
        assert_eq!(
            invoke(&window, "list_harness_results", json!({ "appId": null }))
                .expect("invalid reports were not saved"),
            json!([])
        );
        assert_eq!(
            invoke(
                &window,
                "list_promotion_approvals",
                json!({ "appId": null })
            )
            .expect("invalid approvals were not saved"),
            json!([])
        );
        assert_no_execution_or_copy(directory.path());
    }

    #[test]
    fn promotion_ipc_never_treats_legacy_requirements_as_a_configured_policy() {
        let (directory, manifest, data_dir) = consumer_fixture();
        let app = test_app(data_dir);
        let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
            .build()
            .expect("mock webview");
        let registered = register_reviewed_manifest(&window, &manifest, false);
        let context = promotion_context(&registered, "test", "v1");
        invoke(
            &window,
            "record_harness_result",
            json!({ "request": harness_request(&context, 50, 50) }),
        )
        .expect("report does not require a promotion policy");
        invoke(
            &window,
            "record_consumer_deployment",
            json!({ "request": deployment_request(&context, "files") }),
        )
        .expect("manual deployment");
        let assessment = invoke(&window, "assess_promotion", json!({ "request": context }))
            .expect("unconfigured assessment is a successful IPC result");
        assert_eq!(assessment["status"], "not_configured");
        assert_eq!(assessment["criteria"], json!([]));
        assert_eq!(assessment["stageRequirements"], json!(["checks.passed"]));
        let mut approval = promotion_approval_request(&context, &assessment);
        approval["acknowledgeOptional"] = json!(true);
        assert_request_error(
            &window,
            "approve_promotion",
            approval,
            "promotion_not_configured",
        );
        assert_eq!(
            invoke(
                &window,
                "list_promotion_approvals",
                json!({ "appId": "atlas" })
            )
            .expect("no policy approval saved"),
            json!([])
        );
        assert_no_execution_or_copy(directory.path());
    }

    #[test]
    fn remote_origins_cannot_access_the_local_registry() {
        let (directory, _manifest, data_dir) = consumer_fixture();
        let app = test_app(data_dir.clone());
        let window = tauri::WebviewWindowBuilder::new(app.handle(), "main", Default::default())
            .build()
            .expect("mock webview");
        for (command, args) in [
            ("list_apps", json!({})),
            ("list_pipeline_runs", json!({ "appId": null })),
            (
                "start_pipeline_run",
                json!({
                    "request": {
                        "appId": "atlas",
                        "expectedManifestHash": "0".repeat(64),
                        "expectedUpdatedAt": "2000-01-01T00:00:00Z",
                        "releaseRef": "v1",
                        "approved": true
                    }
                }),
            ),
            ("get_pipeline_run", json!({ "runId": "1" })),
            ("pipeline_run_logs", json!({ "runId": "1", "after": 0 })),
            ("cancel_pipeline_run", json!({ "runId": "1" })),
            (
                "approve_pipeline_stage",
                json!({ "runId": "1", "stageId": "test", "approved": true }),
            ),
            ("list_consumer_deployments", json!({ "appId": null })),
            (
                "record_consumer_deployment",
                json!({
                    "request": {
                        "appId": "atlas",
                        "stageId": "test",
                        "consumerId": "files",
                        "expectedManifestHash": "0".repeat(64),
                        "expectedUpdatedAt": "2000-01-01T00:00:00Z",
                        "releaseRef": "v1",
                        "notes": null,
                        "approved": true
                    }
                }),
            ),
            ("list_harness_results", json!({ "appId": null })),
            (
                "record_harness_result",
                json!({
                    "request": {
                        "appId": "atlas",
                        "stageId": "test",
                        "checkId": "unit",
                        "expectedManifestHash": "0".repeat(64),
                        "expectedUpdatedAt": "2000-01-01T00:00:00Z",
                        "releaseRef": "v1",
                        "total": 50,
                        "passed": 45,
                        "notes": null
                    }
                }),
            ),
            (
                "assess_promotion",
                json!({
                    "request": {
                        "appId": "atlas",
                        "stageId": "test",
                        "expectedManifestHash": "0".repeat(64),
                        "expectedUpdatedAt": "2000-01-01T00:00:00Z",
                        "releaseRef": "v1"
                    }
                }),
            ),
            (
                "approve_promotion",
                json!({
                    "request": {
                        "appId": "atlas",
                        "stageId": "test",
                        "expectedManifestHash": "0".repeat(64),
                        "expectedUpdatedAt": "2000-01-01T00:00:00Z",
                        "releaseRef": "v1",
                        "expectedAssessmentHash": "1".repeat(64),
                        "approved": true,
                        "acknowledgeOptional": true,
                        "notes": null
                    }
                }),
            ),
            ("list_promotion_approvals", json!({ "appId": null })),
        ] {
            let mut request = request(&window, command, args);
            request.url = "https://example.invalid".parse().expect("remote origin");
            assert!(get_ipc_response(&window, request).is_err(), "{command}");
        }
        assert!(!data_dir.exists());
        assert!(!directory.path().join("execution-marker").exists());
        assert!(!directory.path().join("consumer destination").exists());
    }

    #[test]
    fn registry_info_uses_frontend_field_names() {
        let info = RegistryInfo {
            data_dir: "/state".to_owned(),
            database_path: "/state/registry.sqlite3".to_owned(),
            app_version: "0.1.0",
            os: "macos",
            arch: "aarch64",
        };
        let value = serde_json::to_value(info).expect("registry info serializes");
        assert_eq!(value["dataDir"], "/state");
        assert_eq!(value["databasePath"], "/state/registry.sqlite3");
        assert_eq!(value["appVersion"], "0.1.0");
    }

    #[test]
    fn command_errors_are_structured() {
        let error = CommandError {
            code: "manifest_changed".to_owned(),
            message: "Preview the changed manifest again.".to_owned(),
        };
        let value = serde_json::to_value(error).expect("command error serializes");
        assert_eq!(value["code"], "manifest_changed");
        assert_eq!(value["message"], "Preview the changed manifest again.");
    }

    #[cfg(unix)]
    mod runner;
}
