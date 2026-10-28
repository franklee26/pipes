use std::{fs, path::PathBuf, process::Command};

use assert_cmd::prelude::*;
use predicates::prelude::*;
use serde_json::{Value, json};
use tempfile::{Builder, TempDir};

const MINIMAL: &str = "api: pipes/v1\n\
app: demo\n\
source: {path: source}\n\
version: {from: 'file:VERSION'}\n\
stages: [{id: build}]\n";

struct Fixture {
    root: TempDir,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = Builder::new()
            .prefix(".pipes-cli-test-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap();
        fs::create_dir(root.path().join("source")).unwrap();
        fs::write(root.path().join("app.yaml"), MINIMAL).unwrap();
        let state = root.path().join("state");
        Self { root, state }
    }

    fn with_consumers() -> Self {
        let fixture = Self::new();
        let target = json!(fixture.root.path().join("consumer destination"));
        let exec = json!(["touch", fixture.root.path().join("execution-marker")]);
        let yaml = MINIMAL.replace(
            "stages: [{id: build}]",
            &format!(
                "stages:
  - id: build
    checks:
      - id: sentinel
        exec: {exec}
    consumers:
      - id: files
        name: Local artifacts
        target: {target}
        approval: manual
  - id: release
    consumers:
      - id: files
        name: Production
        target: display-only:production
        approval: manual"
            ),
        );
        fs::write(fixture.root.path().join("app.yaml"), yaml).unwrap();
        fs::write(
            fixture.root.path().join("source/artifact.txt"),
            "not copied or deployed",
        )
        .unwrap();
        fixture
    }

    fn with_promotion() -> Self {
        let fixture = Self::with_consumers();
        let manifest = fixture.root.path().join("app.yaml");
        let yaml = fs::read_to_string(&manifest)
            .unwrap()
            .replace("- id: sentinel", "- id: smoke")
            .replace(
                "  - id: release",
                "      - id: personal
        name: Personal device
        target: display-only:personal-device
        approval: manual
    require: [checks.passed]
    promotion:
      criteria:
        - id: smoke-tests
          kind: harness
          check: smoke
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
  - id: release",
            );
        fs::write(manifest, yaml).unwrap();
        fixture
    }

    fn assert_no_execution_or_copy(&self) {
        assert!(!self.root.path().join("execution-marker").exists());
        assert!(!self.root.path().join("consumer destination").exists());
        assert!(!self.root.path().join("replacement destination").exists());
        assert_eq!(
            fs::read_to_string(self.root.path().join("source/artifact.txt")).unwrap(),
            "not copied or deployed"
        );
        assert_eq!(
            fs::read_dir(self.root.path().join("source"))
                .unwrap()
                .count(),
            1
        );
    }

    fn bare_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pipes"));
        command
            .current_dir(self.root.path())
            .env_remove("PIPES_DATA_DIR");
        command
    }

    fn command(&self) -> Command {
        let mut command = self.bare_command();
        command.arg("--data-dir").arg(&self.state);
        command
    }

    fn json(&self, arguments: &[&str]) -> Value {
        let output = self
            .command()
            .arg("--json")
            .args(arguments)
            .assert()
            .success()
            .stderr("")
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice(&output).unwrap()
    }

    fn error(&self, arguments: &[&str], code: &str) -> Value {
        let output = self
            .command()
            .arg("--json")
            .args(arguments)
            .assert()
            .failure()
            .stdout("")
            .get_output()
            .stderr
            .clone();
        let value: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["error"]["code"], code, "{value}");
        assert!(value["error"]["message"].as_str().unwrap().len() > 10);
        value
    }
}

#[test]
fn validate_json_is_the_preview_dto_and_never_creates_a_registry() {
    let fixture = Fixture::new();
    let preview = fixture.json(&["validate", "app.yaml"]);
    assert_eq!(preview["appId"], "demo");
    assert_eq!(preview["name"], "demo");
    assert_eq!(preview["manifestYaml"], MINIMAL);
    assert_eq!(
        preview["execution"],
        json!({"os": null, "arch": null, "timeout": null, "maxParallel": "auto"})
    );
    assert_eq!(preview["stages"][0]["requiresApproval"], false);
    assert_eq!(preview["stages"][0]["after"], json!([]));
    assert_eq!(preview["stages"][0]["consumers"], json!([]));
    assert!(preview.get("manifest").is_none());
    assert!(!fixture.state.exists());
    fixture
        .bare_command()
        .env("PIPES_DATA_DIR", "")
        .args(["validate", "app.yaml", "--json"])
        .assert()
        .success();
    assert!(!fixture.state.exists());
}

#[test]
fn cli_json_round_trip_persists_snapshots_and_unregisters_metadata_only() {
    let fixture = Fixture::new();
    let registered = fixture.json(&["register", "app.yaml"]);
    assert_eq!(registered["manifest"]["appId"], "demo");
    assert!(registered["registeredAt"].is_string());
    assert_eq!(registered["registeredAt"], registered["updatedAt"]);
    assert_eq!(fixture.json(&["register", "app.yaml"]), registered);
    assert_eq!(fixture.json(&["list"]), json!([registered.clone()]));
    fs::rename(
        fixture.root.path().join("app.yaml"),
        fixture.root.path().join("moved.yaml"),
    )
    .unwrap();
    assert_eq!(fixture.json(&["inspect", "demo"]), registered);
    assert_eq!(
        fixture.json(&["unregister", "demo"]),
        json!({"appId": "demo", "unregistered": true})
    );
    assert_eq!(fixture.json(&["list"]), json!([]));
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("moved.yaml")).unwrap(),
        MINIMAL
    );
    assert!(fixture.root.path().join("source").is_dir());
    fixture.error(&["inspect", "demo"], "not_found");
    fixture.error(&["unregister", "demo"], "not_found");
}

#[test]
fn replace_is_explicit_and_path_identity_collisions_still_fail() {
    let fixture = Fixture::new();
    let first = fixture.json(&["register", "app.yaml"]);
    fs::write(
        fixture.root.path().join("app.yaml"),
        format!("{MINIMAL}name: Updated\n"),
    )
    .unwrap();
    fixture.error(&["register", "app.yaml"], "already_registered");
    assert_eq!(fixture.json(&["inspect", "demo"]), first);
    let updated = fixture.json(&["register", "app.yaml", "--replace"]);
    assert_eq!(updated["manifest"]["name"], "Updated");
    assert_eq!(updated["registeredAt"], first["registeredAt"]);
    fs::write(
        fixture.root.path().join("app.yaml"),
        MINIMAL.replace("app: demo", "app: other"),
    )
    .unwrap();
    fixture.error(&["register", "app.yaml", "--replace"], "already_registered");
    assert_eq!(fixture.json(&["inspect", "demo"]), updated);
}

#[test]
fn failure_exit_paths_have_structured_stderr_and_empty_stdout() {
    let fixture = Fixture::new();
    fixture.error(&["validate", "missing.yaml"], "manifest_io");
    assert!(!fixture.state.exists());
    fs::write(
        fixture.root.path().join("app.yaml"),
        format!("{MINIMAL}unknown: true\n"),
    )
    .unwrap();
    let error = fixture.error(&["validate", "app.yaml"], "invalid_manifest");
    assert!(error["error"]["message"].as_str().unwrap().contains("line"));
    assert!(!fixture.state.exists());
    fs::write(fixture.root.path().join("app.yaml"), MINIMAL).unwrap();
    fs::remove_dir(fixture.root.path().join("source")).unwrap();
    fixture.error(&["validate", "app.yaml"], "source_not_found");
    fixture.error(&["inspect", "missing"], "not_found");
}

#[test]
fn usage_errors_are_json_when_requested() {
    let fixture = Fixture::new();
    for arguments in [
        vec!["inspect"],
        vec!["unknown-command"],
        vec!["list", "--unknown-option"],
    ] {
        fixture.error(&arguments, "usage");
    }
    assert!(!fixture.state.exists());
}

#[test]
fn child_environment_override_and_explicit_flag_have_the_expected_precedence() {
    let fixture = Fixture::new();
    let environment_state = fixture.root.path().join("environment state");
    fixture
        .bare_command()
        .env("PIPES_DATA_DIR", &environment_state)
        .args(["--json", "list"])
        .assert()
        .success()
        .stdout("[]\n");
    assert!(environment_state.join("registry.sqlite3").is_file());
    assert!(!fixture.state.exists());
    let unused = fixture.root.path().join("unused override");
    fixture
        .command()
        .env("PIPES_DATA_DIR", &unused)
        .args(["list", "--json"])
        .assert()
        .success()
        .stdout("[]\n");
    assert!(fixture.state.join("registry.sqlite3").is_file());
    assert!(!unused.exists());
}

#[test]
fn empty_environment_override_is_an_error_without_touching_real_user_state() {
    let fixture = Fixture::new();
    let output = fixture
        .bare_command()
        .env("PIPES_DATA_DIR", "")
        .args(["--json", "list"])
        .assert()
        .failure()
        .stdout("")
        .get_output()
        .stderr
        .clone();
    let error: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(error["error"]["code"], "invalid_data_dir");
    assert!(!fixture.state.exists());
}

#[cfg(unix)]
#[test]
fn platform_default_is_used_only_inside_an_isolated_child_home() {
    let fixture = Fixture::new();
    let home = fixture.root.path().join("isolated home");
    let xdg = fixture.root.path().join("isolated data");
    fs::create_dir(&home).unwrap();
    fixture
        .bare_command()
        .env("HOME", &home)
        .env("XDG_DATA_HOME", &xdg)
        .args(["--json", "list"])
        .assert()
        .success()
        .stdout("[]\n");
    #[cfg(target_os = "macos")]
    let expected = home.join("Library/Application Support/dev.pipes.Pipes/registry.sqlite3");
    #[cfg(not(target_os = "macos"))]
    let expected = xdg.join("pipes/registry.sqlite3");
    assert!(expected.is_file(), "{}", expected.display());
}

#[test]
fn human_outputs_are_concise_and_errors_are_nonzero() {
    let fixture = Fixture::new();
    fixture
        .command()
        .args(["validate", "app.yaml"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("Valid: demo (demo)")
                .and(predicate::str::contains("No commands were executed.")),
        );
    fixture
        .command()
        .arg("list")
        .assert()
        .success()
        .stdout("No apps registered.\n");
    fixture
        .command()
        .args(["register", "app.yaml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Registered: demo"));
    fixture
        .command()
        .args(["inspect", "missing"])
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains("error [not_found]"));
    fixture.command().arg("--help").assert().success().stdout(
        predicate::str::contains("Use run --approve")
            .and(predicate::str::contains(
                "stage gates require separate approval",
            ))
            .and(predicate::str::contains("never runs manifest commands").not()),
    );
}

#[test]
fn cli_handles_unicode_manifest_paths_and_does_not_execute_commands() {
    let fixture = Fixture::new();
    let manifest = fixture.root.path().join("Plan 日本 space.yaml");
    let marker = fixture.root.path().join("MUST-NOT-EXIST");
    let script = format!("touch '{}'", marker.display());
    let exec = serde_json::to_string(&["sh", "-c", script.as_str()]).unwrap();
    fs::write(
        &manifest,
        format!("{MINIMAL}prepare: [{{id: sentinel, exec: {exec}}}]\n"),
    )
    .unwrap();
    for command in ["validate", "register"] {
        fixture
            .command()
            .arg("--json")
            .arg(command)
            .arg(&manifest)
            .assert()
            .success();
    }
    fixture.json(&["inspect", "demo"]);
    fixture.json(&["unregister", "demo"]);
    assert!(manifest.is_file());
    assert!(!marker.exists());
}

#[test]
fn storage_failures_are_structured() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.state).unwrap();
    fs::write(fixture.state.join("registry.sqlite3"), "not sqlite").unwrap();
    fixture.error(&["list"], "storage");
}

fn branched_yaml() -> String {
    MINIMAL.replace(
        "[{id: build}]",
        "[{id: release, after: [lint, test], require: [previous.passed]}, \
          {id: build, after: []}, {id: test, after: [build]}, {id: lint, after: [build]}, \
          {id: docs, after: []}]",
    )
}

#[test]
fn graph_json_exposes_resolved_edges_in_declaration_order_on_all_read_surfaces() {
    let fixture = Fixture::new();
    fs::write(fixture.root.path().join("app.yaml"), branched_yaml()).unwrap();
    let preview = fixture.json(&["validate", "app.yaml"]);
    assert_eq!(preview["stages"][0]["id"], "release");
    assert_eq!(preview["stages"][0]["after"], json!(["lint", "test"]));
    assert_eq!(preview["stages"][1]["after"], json!([]));
    assert_eq!(preview["stages"][2]["after"], json!(["build"]));
    assert_eq!(preview["stages"][3]["after"], json!(["build"]));
    assert_eq!(preview["stages"][4]["after"], json!([]));
    assert!(!fixture.state.exists());
    let registered = fixture.json(&["register", "app.yaml"]);
    assert_eq!(registered["manifest"], preview);
    fs::remove_file(fixture.root.path().join("app.yaml")).unwrap();
    assert_eq!(fixture.json(&["inspect", "demo"]), registered);
    assert_eq!(fixture.json(&["list"]), json!([registered]));
}

#[test]
fn human_graph_output_lists_actual_parents_instead_of_a_fictitious_chain() {
    let fixture = Fixture::new();
    fs::write(fixture.root.path().join("app.yaml"), branched_yaml()).unwrap();
    let expected = "Stages (all incoming parents required):
  release: after lint, test
  build: root
  test: after build
  lint: after build
  docs: root
";
    for arguments in [vec!["validate", "app.yaml"], vec!["inspect", "demo"]] {
        if arguments[0] == "inspect" {
            fixture.json(&["register", "app.yaml"]);
            fs::remove_file(fixture.root.path().join("app.yaml")).unwrap();
        }
        fixture.command().args(arguments).assert().success().stdout(
            predicate::str::contains(expected)
                .and(predicate::str::contains("release -> build").not()),
        );
    }
}

#[test]
fn human_linear_output_is_preserved_for_implicit_and_explicit_chains() {
    let fixture = Fixture::new();
    for stages in [
        "[{id: build}, {id: release}]",
        "[{id: build, after: []}, {id: release, after: [build]}]",
    ] {
        fs::write(
            fixture.root.path().join("app.yaml"),
            MINIMAL.replace("[{id: build}]", stages),
        )
        .unwrap();
        fixture
            .command()
            .args(["validate", "app.yaml"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("Stages: build -> release\n")
                    .and(predicate::str::contains("all incoming parents").not()),
            );
    }
}

#[test]
fn invalid_graphs_are_structured_failures_without_creating_registry_state() {
    let fixture = Fixture::new();
    for stages in [
        "[{id: build, after: null}]",
        "[{id: build, after: [missing]}]",
        "[{id: a, after: [b]}, {id: b}]",
    ] {
        fs::write(
            fixture.root.path().join("app.yaml"),
            MINIMAL.replace("[{id: build}]", stages),
        )
        .unwrap();
        let error = fixture.error(&["validate", "app.yaml"], "invalid_manifest");
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("after")
        );
        assert!(!fixture.state.exists());
    }
}

fn planned_checks_yaml() -> String {
    format!(
        "{}execution: {{max_parallel: 3}}\n",
        MINIMAL.replace(
            "stages: [{id: build}]",
            r#"stages:
  - id: build
    require: [checks.passed]
    checks:
      - id: batch-a
        max_parallel: 2
        parallel:
          - {id: a-one, exec: [test-tool, a-one]}
          - id: chained
            sequence:
              - {id: a-two, exec: [test-tool, a-two]}
              - id: nested
                max_parallel: 8
                parallel:
                  - {id: a-three, exec: [test-tool, a-three]}
      - id: batch-b
        max_parallel: 6
        parallel:
          - {id: b-one, exec: [test-tool, b-one]}"#,
        )
    )
}

#[test]
fn human_validate_register_and_inspect_preserve_barriers_and_show_effective_command_caps() {
    let fixture = Fixture::new();
    fs::write(fixture.root.path().join("app.yaml"), planned_checks_yaml()).unwrap();
    let tree = "Checks build (sequence; advance only after each child passes):
  1. parallel batch-a (all required; cap 2; effective <= 2 commands):
    1. command a-one
    2. sequence chained (all required):
      1. command a-two
      2. parallel nested (all required; cap 8; effective <= 2 commands):
        1. command a-three
  2. parallel batch-b (all required; cap 6; effective <= 3 commands):
    1. command b-one
";
    for (command, target) in [
        ("validate", "app.yaml"),
        ("register", "app.yaml"),
        ("inspect", "demo"),
    ] {
        if command == "inspect" {
            fs::remove_file(fixture.root.path().join("app.yaml")).unwrap();
        }
        fixture
            .command()
            .args([command, target])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("Command budget: <= 3 commands (shared run-wide)")
                    .and(predicate::str::contains(tree))
                    .and(predicate::str::contains("effective <= 16").not()),
            );
    }
}

#[test]
fn cli_json_keeps_declared_caps_and_depth_first_checks_without_executing_nested_leaves() {
    let fixture = Fixture::new();
    let marker = fixture.root.path().join("MUST-NOT-EXIST");
    let script = format!("touch '{}'", marker.display());
    let exec = serde_json::to_string(&["sh", "-c", script.as_str()]).unwrap();
    let yaml = planned_checks_yaml().replace("[test-tool, a-three]", &exec);
    fs::write(fixture.root.path().join("app.yaml"), &yaml).unwrap();
    let preview = fixture.json(&["validate", "app.yaml"]);
    assert_eq!(preview["execution"]["maxParallel"], 3);
    let plan = &preview["stages"][0]["checkPlan"];
    assert_eq!(plan[0]["kind"], "parallel");
    assert_eq!(plan[0]["maxParallel"], 2);
    assert_eq!(plan[0]["steps"][1]["kind"], "sequence");
    assert_eq!(plan[0]["steps"][1]["steps"][1]["maxParallel"], 8);
    assert_eq!(plan[1]["maxParallel"], 6);
    let checks: Vec<_> = preview["stages"][0]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|command| command["id"].as_str().unwrap())
        .collect();
    assert_eq!(checks, ["a-one", "a-two", "a-three", "b-one"]);
    assert!(!fixture.state.exists());
    let registered = fixture.json(&["register", "app.yaml"]);
    assert_eq!(registered["manifest"], preview);
    fs::remove_file(fixture.root.path().join("app.yaml")).unwrap();
    assert_eq!(fixture.json(&["inspect", "demo"]), registered);
    assert_eq!(fixture.json(&["list"]), json!([registered]));
    fixture.json(&["unregister", "demo"]);
    assert!(!marker.exists());
}

#[test]
fn auto_budget_and_inherited_group_caps_remain_unresolved_in_json_and_human_output() {
    let fixture = Fixture::new();
    let yaml = MINIMAL.replace(
        "[{id: build}]",
        "[{id: build, checks: [{id: batch, parallel: [{id: one, exec: [tool]}]}]}]",
    );
    fs::write(fixture.root.path().join("app.yaml"), yaml).unwrap();
    let preview = fixture.json(&["validate", "app.yaml"]);
    assert_eq!(preview["execution"]["maxParallel"], "auto");
    assert_eq!(
        preview["stages"][0]["checkPlan"][0]["maxParallel"],
        Value::Null
    );
    fixture.command()
        .args(["validate", "app.yaml"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("Command budget: auto (host capacity unresolved) (shared run-wide)")
                .and(predicate::str::contains("parallel batch (all required; cap inherit; effective auto (host capacity unresolved))")),
        );
}

#[test]
fn simple_flat_checks_still_have_concise_sequential_human_output() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.path().join("app.yaml"),
        MINIMAL.replace(
            "[{id: build}]",
            "[{id: build, checks: [{id: one, exec: [tool]}, {id: two, exec: [tool]}]}]",
        ),
    )
    .unwrap();
    fixture
        .command()
        .args(["validate", "app.yaml"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("Checks build (sequence): one -> two")
                .and(predicate::str::contains("parallel ").not()),
        );
}

#[test]
fn invalid_plans_and_budgets_are_structured_errors_and_run_requires_explicit_arguments() {
    let fixture = Fixture::new();
    for yaml in [
        format!("{MINIMAL}execution: {{max_parallel: '2'}}\n"),
        MINIMAL.replace("[{id: build}]", "[{id: build, checks: [{id: batch, parallel: null}]}]"),
        MINIMAL.replace("[{id: build}]", "[{id: build, checks: [{id: batch, parallel: [{id: one, exec: [tool]}], max_parallel: 0}]}]"),
        MINIMAL.replace("[{id: build}]", "[{id: build, checks: [{id: batch, parallel: [{id: batch, exec: [tool]}]}]}]"),
    ] {
        fs::write(fixture.root.path().join("app.yaml"), yaml).unwrap();
        fixture.error(&["validate", "app.yaml"], "invalid_manifest");
        assert!(!fixture.state.exists());
    }

    fixture.error(&["run", "demo"], "usage");
    assert!(!fixture.state.exists());
}

fn deployment_arguments<'a>(
    app_id: &'a str,
    stage: &'a str,
    consumer: &'a str,
    release_ref: &'a str,
) -> Vec<&'a str> {
    vec![
        "deployed-to",
        app_id,
        "--stage",
        stage,
        "--consumer",
        consumer,
        "--release-ref",
        release_ref,
        "--approve",
    ]
}

#[test]
fn consumer_cli_round_trip_preserves_stage_destinations_and_never_deploys() {
    let fixture = Fixture::with_consumers();
    let preview = fixture.json(&["validate", "app.yaml"]);
    assert_eq!(
        preview["stages"][0]["consumers"],
        json!([{
            "id": "files",
            "name": "Local artifacts",
            "target": fixture.root.path().join("consumer destination"),
            "approval": "manual"
        }])
    );
    assert_eq!(preview["stages"][0]["requiresApproval"], false);
    assert_eq!(preview["stages"][1]["consumers"][0]["id"], "files");
    assert!(!fixture.state.exists());
    for (command, target) in [
        ("validate", "app.yaml"),
        ("register", "app.yaml"),
        ("inspect", "demo"),
    ] {
        fixture
                .command()
                .args([command, target])
                .assert()
                .success()
                .stderr("")
                .stdout(
                    predicate::str::contains("Consumers build:\n  Local artifacts (files) -> ")
                        .and(predicate::str::contains("consumer destination (manual approval)"))
                        .and(predicate::str::contains(
                            "Consumers release:\n  Production (files) -> display-only:production (manual approval)",
                        ))
                        .and(predicate::str::contains("Stages: build -> release"))
                        .and(predicate::str::contains("Checks build (sequence): sentinel")),
                );
    }
    let registration = fixture.json(&["inspect", "demo"]);
    fixture
        .command()
        .arg("deployments")
        .assert()
        .success()
        .stderr("")
        .stdout("No manual deployments recorded.\n");
    assert_eq!(fixture.json(&["deployments", "demo"]), json!([]));

    let mut arguments = deployment_arguments("demo", "build", "files", "refs/tags/v1");
    arguments.extend(["--note", "Verified outside Pipes"]);
    let deployment = fixture.json(&arguments);
    assert!(!deployment["id"].as_str().unwrap().is_empty());
    assert!(!deployment["recordedAt"].as_str().unwrap().is_empty());
    assert_eq!(
        deployment,
        json!({
            "id": deployment["id"],
            "appId": "demo",
            "appName": "demo",
            "stageId": "build",
            "consumer": preview["stages"][0]["consumers"][0],
            "releaseRef": "refs/tags/v1",
            "notes": "Verified outside Pipes",
            "manifestHash": preview["manifestHash"],
            "manifestPath": preview["manifestPath"],
            "sourcePath": preview["sourcePath"],
            "registrationUpdatedAt": registration["updatedAt"],
            "recordedAt": deployment["recordedAt"]
        })
    );
    fixture
        .command()
        .args(deployment_arguments(
            "demo",
            "release",
            "files",
            "refs/tags/v2",
        ))
        .assert()
        .success()
        .stderr("")
        .stdout(
            predicate::str::contains("Recorded manual deployment:")
                .and(predicate::str::contains("Source stage: release"))
                .and(predicate::str::contains("Consumer: Production (files)"))
                .and(predicate::str::contains("Target: display-only:production"))
                .and(predicate::str::contains("Ref: refs/tags/v2"))
                .and(predicate::str::contains("Approval: manual"))
                .and(predicate::str::contains(
                    "No files were copied or deployed. No commands were executed.",
                )),
        );
    let history = fixture.json(&["deployments"]);
    assert_eq!(history.as_array().unwrap().len(), 2);
    assert_eq!(history[1], deployment);
    assert_eq!(history[0]["stageId"], "release");
    assert_eq!(history[0]["consumer"], preview["stages"][1]["consumers"][0]);
    assert_eq!(history[0]["releaseRef"], "refs/tags/v2");
    assert_eq!(history[0]["notes"], Value::Null);
    assert_ne!(history[0]["id"], history[1]["id"]);
    assert_eq!(fixture.json(&["deployments", "demo"]), history);
    assert_eq!(fixture.json(&["deployments", "unknown"]), json!([]));
    fixture
        .command()
        .args(["deployments", "demo"])
        .assert()
        .success()
        .stderr("")
        .stdout(
            predicate::str::contains("Manual deployment history (newest first")
                .and(predicate::str::contains(format!(
                    "ID: {}",
                    deployment["id"].as_str().unwrap()
                )))
                .and(predicate::str::contains(format!(
                    "Recorded: {}",
                    deployment["recordedAt"].as_str().unwrap()
                )))
                .and(predicate::str::contains("App: demo (demo)"))
                .and(predicate::str::contains("Source stage: build"))
                .and(predicate::str::contains("consumer destination"))
                .and(predicate::str::contains("Ref: refs/tags/v1"))
                .and(predicate::str::contains("Approval: manual")),
        );
    fixture.assert_no_execution_or_copy();
}

#[test]
fn deployment_approval_and_review_token_pair_are_required_usage_before_opening_registry() {
    let fixture = Fixture::with_consumers();
    let arguments = deployment_arguments("demo", "build", "files", "v1");
    let mut without_approval = arguments.clone();
    without_approval.pop();
    let mut hash_only = arguments.clone();
    hash_only.extend(["--expected-hash", "previously-reviewed-hash"]);
    let mut timestamp_only = arguments.clone();
    timestamp_only.extend(["--expected-updated-at", "2000-01-01T00:00:00Z"]);
    let mut false_approval = without_approval.clone();
    false_approval.push("--approve=false");
    for arguments in [without_approval, hash_only, timestamp_only, false_approval] {
        let output = fixture
            .command()
            .arg("--json")
            .args(arguments)
            .assert()
            .code(2)
            .stdout("")
            .get_output()
            .stderr
            .clone();
        let error: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(error["error"]["code"], "usage");
        assert!(error["error"]["message"].is_string());
    }
    assert!(!fixture.state.exists());
    fixture.assert_no_execution_or_copy();
}

#[test]
fn deployment_errors_preserve_structured_stderr_and_enforce_ref_and_note_limits() {
    let fixture = Fixture::with_consumers();
    fixture.json(&["register", "app.yaml"]);
    let oversized_ref = "r".repeat(257);
    for (stage, consumer, release_ref, code) in [
        ("missing", "files", "v1", "consumer_not_found"),
        ("build", "missing", "v1", "consumer_not_found"),
        ("build", "files", "", "invalid_deployment"),
        ("build", "files", " \t ", "invalid_deployment"),
        (
            "build",
            "files",
            oversized_ref.as_str(),
            "invalid_deployment",
        ),
    ] {
        fixture.error(
            &deployment_arguments("demo", stage, consumer, release_ref),
            code,
        );
    }
    fixture.error(
        &deployment_arguments("unknown", "build", "files", "v1"),
        "not_found",
    );
    let oversized_note = "n".repeat(4097);
    let mut arguments = deployment_arguments("demo", "build", "files", "v1");
    arguments.extend(["--note", &oversized_note]);
    fixture.error(&arguments, "invalid_deployment");
    fixture
        .command()
        .args(deployment_arguments("demo", "build", "missing", "v1"))
        .assert()
        .code(1)
        .stdout("")
        .stderr(predicate::str::contains("error [consumer_not_found]"));
    assert_eq!(fixture.json(&["deployments"]), json!([]));

    let maximum_ref = "r".repeat(256);
    let maximum_note = "n".repeat(4096);
    let mut arguments = deployment_arguments("demo", "build", "files", &maximum_ref);
    arguments.extend(["--note", &maximum_note]);
    let deployment = fixture.json(&arguments);
    assert_eq!(deployment["releaseRef"], maximum_ref);
    assert_eq!(deployment["notes"], maximum_note);
    fixture.assert_no_execution_or_copy();
}

#[test]
fn deployment_history_survives_replacement_consumer_removal_and_unregistration() {
    let fixture = Fixture::with_consumers();
    let original = fixture.json(&["register", "app.yaml"]);
    let original_hash = original["manifest"]["manifestHash"].as_str().unwrap();
    let original_updated_at = original["updatedAt"].as_str().unwrap();
    let mut arguments = deployment_arguments("demo", "build", "files", "v1");
    arguments.extend([
        "--expected-hash",
        original_hash,
        "--expected-updated-at",
        original_updated_at,
    ]);
    let first = fixture.json(&arguments);
    let original_yaml = original["manifest"]["manifestYaml"].as_str().unwrap();
    let updated_yaml = original_yaml
        .replace("app: demo", "app: demo\nname: Updated demo")
        .replace("name: Local artifacts", "name: Updated artifacts")
        .replace("consumer destination", "replacement destination");
    fs::write(fixture.root.path().join("app.yaml"), updated_yaml).unwrap();
    let updated = fixture.json(&["register", "app.yaml", "--replace"]);
    let updated_hash = updated["manifest"]["manifestHash"].as_str().unwrap();
    let updated_at = updated["updatedAt"].as_str().unwrap();
    assert_ne!(original_hash, updated_hash);
    assert_ne!(original_updated_at, updated_at);
    for (hash, timestamp) in [
        (original_hash, updated_at),
        (updated_hash, original_updated_at),
    ] {
        let mut arguments = deployment_arguments("demo", "build", "files", "v2");
        arguments.extend(["--expected-hash", hash, "--expected-updated-at", timestamp]);
        fixture.error(&arguments, "registration_changed");
    }
    let mut arguments = deployment_arguments("demo", "build", "files", "v2");
    arguments.extend([
        "--expected-hash",
        updated_hash,
        "--expected-updated-at",
        updated_at,
    ]);
    let second = fixture.json(&arguments);
    assert_eq!(second["appName"], "Updated demo");
    assert_eq!(
        second["consumer"]["target"],
        json!(fixture.root.path().join("replacement destination"))
    );
    let history = json!([second, first]);

    fs::write(fixture.root.path().join("app.yaml"), MINIMAL).unwrap();
    fixture.json(&["register", "app.yaml", "--replace"]);
    fixture.error(
        &deployment_arguments("demo", "build", "files", "v3"),
        "consumer_not_found",
    );
    assert_eq!(fixture.json(&["deployments", "demo"]), history);
    fixture
        .command()
        .args(["unregister", "demo"])
        .assert()
        .success()
        .stderr("")
        .stdout(
            predicate::str::contains("Unregistered 'demo'. Deployment history was retained.").and(
                predicate::str::contains("Source and manifest files were not changed."),
            ),
        );
    assert_eq!(fixture.json(&["list"]), json!([]));
    assert_eq!(fixture.json(&["deployments", "demo"]), history);
    assert_eq!(fixture.json(&["deployments"]), history);
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("app.yaml")).unwrap(),
        MINIMAL
    );
    fixture.error(
        &deployment_arguments("demo", "build", "files", "v4"),
        "not_found",
    );

    fs::write(
        fixture.root.path().join("app.yaml"),
        original_yaml.replace("app: demo", "app: another"),
    )
    .unwrap();
    fixture.json(&["register", "app.yaml"]);
    let other = fixture.json(&deployment_arguments("another", "build", "files", "v5"));
    assert_eq!(
        fixture.json(&["deployments"]),
        json!([other, history[0], history[1]])
    );
    assert_eq!(fixture.json(&["deployments", "demo"]), history);
    fixture.assert_no_execution_or_copy();
}

fn promotion_arguments<'a>(
    command: &'a str,
    app_id: &'a str,
    stage: &'a str,
    release_ref: &'a str,
) -> Vec<&'a str> {
    vec![
        command,
        app_id,
        "--stage",
        stage,
        "--release-ref",
        release_ref,
    ]
}

fn harness_arguments<'a>(
    app_id: &'a str,
    stage: &'a str,
    check: &'a str,
    release_ref: &'a str,
    total: &'a str,
    passed: &'a str,
) -> Vec<&'a str> {
    let mut arguments = promotion_arguments("report-harness", app_id, stage, release_ref);
    arguments.extend(["--check", check, "--total", total, "--passed", passed]);
    arguments
}

fn promotion_criteria() -> Value {
    json!([
        {
            "kind": "harness",
            "id": "smoke-tests",
            "checkId": "smoke",
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
    ])
}

#[test]
fn promotion_policy_definitions_are_preserved_on_all_manifest_surfaces() {
    let fixture = Fixture::with_promotion();
    let preview = fixture.json(&["validate", "app.yaml"]);
    assert_eq!(
        preview["stages"][0]["promotionCriteria"],
        promotion_criteria()
    );
    assert_eq!(preview["stages"][1]["promotionCriteria"], json!([]));
    assert!(!fixture.state.exists());
    for (command, target) in [
        ("validate", "app.yaml"),
        ("register", "app.yaml"),
        ("inspect", "demo"),
    ] {
        if command == "inspect" {
            fs::remove_file(fixture.root.path().join("app.yaml")).unwrap();
        }
        fixture
            .command()
            .args([command, target])
            .assert()
            .success()
            .stderr("")
            .stdout(
                predicate::str::contains("Promotion criteria build:")
                    .and(predicate::str::contains(
                        "smoke-tests: harness smoke; exactly 50 tests, minimum 45 passed",
                    ))
                    .and(predicate::str::contains(
                        "device-required: required manual deployment to files",
                    ))
                    .and(predicate::str::contains(
                        "personal-optional: optional manual deployment to personal",
                    ))
                    .and(predicate::str::contains(
                        "Stage requirements build (not evaluated in this promotion assessment): checks.passed",
                    ))
                    .and(predicate::str::contains("Stages: build -> release"))
                    .and(predicate::str::contains("Checks build (sequence): smoke"))
                    .and(predicate::str::contains("Consumers build:"))
                    .and(predicate::str::contains(
                        "Promotion criteria release: not configured.",
                    )),
            );
    }
    assert_eq!(fixture.json(&["inspect", "demo"])["manifest"], preview);
    fixture.assert_no_execution_or_copy();
}

#[test]
fn promotion_cli_round_trip_persists_exact_dtos_skipped_history_and_archived_decisions() {
    let fixture = Fixture::with_promotion();
    let registration = fixture.json(&["register", "app.yaml"]);
    let preview = &registration["manifest"];
    let status_arguments = promotion_arguments("promotion-status", "demo", "build", "v1");
    let mut approve_arguments = promotion_arguments("approve-promotion", "demo", "build", "v1");
    approve_arguments.push("--approve");
    let initial = fixture.json(&status_arguments);
    assert_eq!(initial["status"], "blocked");
    assert_eq!(initial["criteria"][0]["observed"], Value::Null);
    assert_eq!(initial["criteria"][1]["status"], "blocked");
    assert_eq!(initial["criteria"][2]["status"], "warning");

    let mut report_arguments = harness_arguments("demo", "build", "smoke", "v1", "50", "45");
    report_arguments.extend(["--note", "External CI report"]);
    let reported = fixture.json(&report_arguments);
    assert!(!reported["id"].as_str().unwrap().is_empty());
    assert!(!reported["recordedAt"].as_str().unwrap().is_empty());
    assert_eq!(
        reported,
        json!({
            "id": reported["id"],
            "appId": "demo",
            "appName": "demo",
            "stageId": "build",
            "checkId": "smoke",
            "releaseRef": "v1",
            "total": 50,
            "passed": 45,
            "notes": "External CI report",
            "manifestHash": preview["manifestHash"],
            "manifestPath": preview["manifestPath"],
            "sourcePath": preview["sourcePath"],
            "registrationUpdatedAt": registration["updatedAt"],
            "recordedAt": reported["recordedAt"]
        })
    );
    fixture.error(&approve_arguments, "promotion_blocked");
    let required = fixture.json(&deployment_arguments("demo", "build", "files", "v1"));
    let ready = fixture.json(&status_arguments);
    assert!(!ready["assessmentHash"].as_str().unwrap().is_empty());
    let criteria = promotion_criteria();
    assert_eq!(
        ready,
        json!({
            "appId": "demo",
            "appName": "demo",
            "stageId": "build",
            "releaseRef": "v1",
            "manifestHash": preview["manifestHash"],
            "manifestPath": preview["manifestPath"],
            "sourcePath": preview["sourcePath"],
            "registrationUpdatedAt": registration["updatedAt"],
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
    for criterion in ready["criteria"].as_array().unwrap() {
        assert!(!criterion["message"].as_str().unwrap().is_empty());
    }
    fixture.error(&approve_arguments, "optional_steps_unacknowledged");
    assert_eq!(fixture.json(&["promotion-approvals"]), json!([]));
    let mut acknowledged_arguments = approve_arguments.clone();
    acknowledged_arguments.extend([
        "--acknowledge-optional",
        "--expected-assessment-hash",
        ready["assessmentHash"].as_str().unwrap(),
        "--note",
        "Personal device deliberately skipped",
    ]);
    let approved = fixture.json(&acknowledged_arguments);
    assert!(!approved["id"].as_str().unwrap().is_empty());
    assert!(!approved["recordedAt"].as_str().unwrap().is_empty());
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
    let failure = fixture.json(&harness_arguments(
        "demo", "build", "smoke", "v1", "50", "44",
    ));
    fixture.json(&deployment_arguments("demo", "build", "personal", "v1"));
    assert_eq!(fixture.json(&status_arguments)["status"], "blocked");
    assert_eq!(
        fixture.json(&["promotion-approvals", "demo"]),
        json!([approved])
    );
    fixture.error(&approve_arguments, "promotion_blocked");
    let recovered = fixture.json(&harness_arguments(
        "demo", "build", "smoke", "v1", "50", "45",
    ));
    let fully_ready = fixture.json(&status_arguments);
    assert_eq!(fully_ready["status"], "ready");
    let second = fixture.json(&approve_arguments);
    assert_eq!(second["assessment"], fully_ready);
    assert_eq!(second["skippedOptional"], json!([]));
    assert_eq!(second["notes"], Value::Null);
    let reports = json!([recovered, failure, reported]);
    let approvals = json!([second, approved]);
    fixture.json(&["unregister", "demo"]);
    for (command, history) in [
        ("harness-results", &reports),
        ("promotion-approvals", &approvals),
    ] {
        assert_eq!(&fixture.json(&[command]), history);
        assert_eq!(&fixture.json(&[command, "demo"]), history);
        assert_eq!(fixture.json(&[command, "unknown"]), json!([]));
    }
    fixture.error(&status_arguments, "not_found");
    fixture.error(&approve_arguments, "not_found");
    fixture.error(&report_arguments, "not_found");
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("app.yaml")).unwrap(),
        preview["manifestYaml"].as_str().unwrap()
    );
    fixture.assert_no_execution_or_copy();
}

#[test]
fn harness_readiness_requires_exact_total_minimum_passes_and_the_latest_result() {
    let fixture = Fixture::with_promotion();
    fixture.json(&["register", "app.yaml"]);
    fixture.json(&deployment_arguments("demo", "build", "files", "v1"));
    for (total, passed, expected_status) in [
        ("50", "44", "blocked"),
        ("49", "45", "blocked"),
        ("51", "50", "blocked"),
        ("0", "0", "blocked"),
        ("50", "45", "ready_with_warnings"),
        ("50", "50", "ready_with_warnings"),
        ("50", "0", "blocked"),
    ] {
        let result = fixture.json(&harness_arguments(
            "demo", "build", "smoke", "v1", total, passed,
        ));
        let assessment = fixture.json(&promotion_arguments(
            "promotion-status",
            "demo",
            "build",
            "v1",
        ));
        assert_eq!(assessment["status"], expected_status, "{total}/{passed}");
        assert_eq!(assessment["criteria"][0]["evidenceId"], result["id"]);
        assert_eq!(
            assessment["criteria"][0]["observed"],
            json!({"total": total.parse::<u32>().unwrap(), "passed": passed.parse::<u32>().unwrap()})
        );
    }
    assert_eq!(
        fixture.json(&["harness-results"]).as_array().unwrap().len(),
        7
    );
    fixture.assert_no_execution_or_copy();
}

#[test]
fn promotion_evidence_and_reviews_are_bound_to_candidate_stage_and_registration() {
    let fixture = Fixture::with_promotion();
    let original = fixture.json(&["register", "app.yaml"]);
    fixture.json(&harness_arguments(
        "demo", "build", "smoke", "other", "50", "45",
    ));
    fixture.json(&deployment_arguments("demo", "build", "files", "other"));
    let status_arguments = promotion_arguments("promotion-status", "demo", "build", "v1");
    let assessment = fixture.json(&status_arguments);
    assert_eq!(assessment["criteria"][0]["evidenceId"], Value::Null);
    assert_eq!(assessment["criteria"][1]["evidenceId"], Value::Null);
    fixture.json(&harness_arguments(
        "demo", "build", "smoke", "v1", "50", "45",
    ));
    fixture.json(&deployment_arguments("demo", "release", "files", "v1"));
    assert_eq!(fixture.json(&status_arguments)["status"], "blocked");
    fixture.json(&deployment_arguments("demo", "build", "files", "v1"));
    assert_eq!(
        fixture.json(&status_arguments)["status"],
        "ready_with_warnings"
    );

    fs::write(
        fixture.root.path().join("app.yaml"),
        original["manifest"]["manifestYaml"]
            .as_str()
            .unwrap()
            .replace("app: demo", "app: demo\nname: Updated demo"),
    )
    .unwrap();
    let current = fixture.json(&["register", "app.yaml", "--replace"]);
    let assessment = fixture.json(&status_arguments);
    assert_eq!(assessment["status"], "blocked");
    assert_eq!(assessment["criteria"][0]["evidenceId"], Value::Null);
    assert_eq!(assessment["criteria"][1]["evidenceId"], Value::Null);
    let mut approval = promotion_arguments("approve-promotion", "demo", "build", "v1");
    approval.extend(["--approve", "--acknowledge-optional"]);
    for arguments in [
        harness_arguments("demo", "build", "smoke", "v1", "50", "45"),
        status_arguments,
        approval,
    ] {
        for (hash, updated_at) in [
            (&original["manifest"]["manifestHash"], &current["updatedAt"]),
            (&current["manifest"]["manifestHash"], &original["updatedAt"]),
        ] {
            let mut stale = arguments.clone();
            stale.extend([
                "--expected-hash",
                hash.as_str().unwrap(),
                "--expected-updated-at",
                updated_at.as_str().unwrap(),
            ]);
            fixture.error(&stale, "registration_changed");
        }
    }
    let mut current_report = harness_arguments("demo", "build", "smoke", "v1", "50", "45");
    current_report.extend([
        "--expected-hash",
        current["manifest"]["manifestHash"].as_str().unwrap(),
        "--expected-updated-at",
        current["updatedAt"].as_str().unwrap(),
    ]);
    assert_eq!(fixture.json(&current_report)["appName"], "Updated demo");
    fixture.assert_no_execution_or_copy();
}

#[test]
fn approval_honors_the_callers_assessment_hash_instead_of_refreshing_it() {
    let fixture = Fixture::with_promotion();
    fixture.json(&["register", "app.yaml"]);
    fixture.json(&harness_arguments(
        "demo", "build", "smoke", "v1", "50", "45",
    ));
    fixture.json(&deployment_arguments("demo", "build", "files", "v1"));
    let status_arguments = promotion_arguments("promotion-status", "demo", "build", "v1");
    let previous = fixture.json(&status_arguments);
    fixture.json(&deployment_arguments("demo", "build", "personal", "v1"));
    let current = fixture.json(&status_arguments);
    assert_ne!(previous["assessmentHash"], current["assessmentHash"]);
    let mut stale = promotion_arguments("approve-promotion", "demo", "build", "v1");
    stale.extend([
        "--approve",
        "--acknowledge-optional",
        "--expected-assessment-hash",
        previous["assessmentHash"].as_str().unwrap(),
    ]);
    fixture.error(&stale, "assessment_changed");
    assert_eq!(fixture.json(&["promotion-approvals"]), json!([]));
    let mut fresh = promotion_arguments("approve-promotion", "demo", "build", "v1");
    fresh.extend([
        "--approve",
        "--expected-assessment-hash",
        current["assessmentHash"].as_str().unwrap(),
    ]);
    assert_eq!(fixture.json(&fresh)["assessment"], current);
    fixture.assert_no_execution_or_copy();
}

#[test]
fn promotion_usage_errors_reject_unpaired_reviews_missing_approval_and_invalid_u32_counts() {
    let fixture = Fixture::with_promotion();
    let report = harness_arguments("demo", "build", "smoke", "v1", "50", "45");
    let status = promotion_arguments("promotion-status", "demo", "build", "v1");
    let approval = promotion_arguments("approve-promotion", "demo", "build", "v1");
    let mut approved = approval.clone();
    approved.push("--approve");
    let mut invalid_arguments = vec![approval.clone()];
    let mut false_approval = approval;
    false_approval.push("--approve=false");
    invalid_arguments.push(false_approval);
    let mut report_with_approval = report.clone();
    report_with_approval.push("--approve");
    invalid_arguments.push(report_with_approval);
    for arguments in [report, status, approved] {
        let mut hash_only = arguments.clone();
        hash_only.extend(["--expected-hash", "previous-hash"]);
        invalid_arguments.push(hash_only);
        let mut timestamp_only = arguments;
        timestamp_only.extend(["--expected-updated-at", "2000-01-01T00:00:00Z"]);
        invalid_arguments.push(timestamp_only);
    }
    for (total, passed) in [
        ("-1", "0"),
        ("4294967296", "0"),
        ("50", "-1"),
        ("50", "4294967296"),
        ("50.5", "45"),
        ("50", "forty-five"),
    ] {
        invalid_arguments.push(harness_arguments(
            "demo", "build", "smoke", "v1", total, passed,
        ));
    }
    invalid_arguments.push(vec!["report-harness", "demo", "--stage", "build"]);
    invalid_arguments.push(vec!["promotion-status", "demo"]);
    for arguments in invalid_arguments {
        let output = fixture
            .command()
            .arg("--json")
            .args(arguments)
            .assert()
            .code(2)
            .stdout("")
            .get_output()
            .stderr
            .clone();
        let error: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(error["error"]["code"], "usage");
        assert!(error["error"]["message"].is_string());
    }
    assert!(!fixture.state.exists());
    fixture.assert_no_execution_or_copy();
}

#[test]
fn promotion_adapter_preserves_core_errors_and_validates_report_counts() {
    let fixture = Fixture::with_promotion();
    fixture.json(&["register", "app.yaml"]);
    fixture.error(
        &harness_arguments("demo", "build", "smoke", "v1", "50", "51"),
        "invalid_harness_result",
    );
    fixture.error(
        &harness_arguments("demo", "build", "unknown", "v1", "50", "45"),
        "check_not_found",
    );
    fixture.error(
        &harness_arguments("demo", "unknown", "smoke", "v1", "50", "45"),
        "stage_not_found",
    );
    fixture.error(
        &harness_arguments("unknown", "build", "smoke", "v1", "50", "45"),
        "not_found",
    );
    fixture.error(
        &promotion_arguments("promotion-status", "demo", "missing", "v1"),
        "stage_not_found",
    );
    fixture.error(
        &promotion_arguments("promotion-status", "demo", "build", " \t "),
        "invalid_promotion",
    );
    assert_eq!(fixture.json(&["harness-results"]), json!([]));
    assert_eq!(fixture.json(&["promotion-approvals"]), json!([]));
    fixture.assert_no_execution_or_copy();
}

#[test]
fn legacy_requirements_and_reported_success_do_not_create_an_implicit_promotion_policy() {
    let fixture = Fixture::with_consumers();
    let manifest = fixture.root.path().join("app.yaml");
    let yaml = fs::read_to_string(&manifest).unwrap().replace(
        "  - id: build\n",
        "  - id: build\n    require: [checks.passed]\n",
    );
    fs::write(&manifest, yaml).unwrap();
    fixture.json(&["register", "app.yaml"]);
    fixture.json(&harness_arguments(
        "demo", "build", "sentinel", "v1", "50", "50",
    ));
    fixture.json(&deployment_arguments("demo", "build", "files", "v1"));
    let assessment = fixture.json(&promotion_arguments(
        "promotion-status",
        "demo",
        "build",
        "v1",
    ));
    assert_eq!(assessment["status"], "not_configured");
    assert_eq!(assessment["criteria"], json!([]));
    assert_eq!(assessment["stageRequirements"], json!(["checks.passed"]));
    let mut approval = promotion_arguments("approve-promotion", "demo", "build", "v1");
    approval.extend(["--approve", "--acknowledge-optional"]);
    fixture.error(&approval, "promotion_not_configured");
    assert_eq!(fixture.json(&["promotion-approvals"]), json!([]));
    fixture.assert_no_execution_or_copy();
}

#[test]
fn human_promotion_output_explains_counts_warnings_skips_and_runner_policy_gates() {
    let fixture = Fixture::with_promotion();
    fixture.json(&["register", "app.yaml"]);
    for (command, message) in [
        ("harness-results", "No external harness results recorded.\n"),
        (
            "promotion-approvals",
            "No promotion-criteria approvals recorded.\n",
        ),
    ] {
        fixture
            .command()
            .arg(command)
            .assert()
            .success()
            .stderr("")
            .stdout(message);
    }
    fixture
        .command()
        .args(harness_arguments(
            "demo", "build", "smoke", "v1", "50", "45",
        ))
        .assert()
        .success()
        .stderr("")
        .stdout(
            predicate::str::contains("Recorded external harness result:")
                .and(predicate::str::contains("Reported: 45/50 passed"))
                .and(predicate::str::contains(
                    "No harness was executed or independently verified.",
                )),
        );
    fixture.json(&deployment_arguments("demo", "build", "files", "v1"));
    let status_arguments = promotion_arguments("promotion-status", "demo", "build", "v1");
    fixture
        .command()
        .args(&status_arguments)
        .assert()
        .success()
        .stderr("")
        .stdout(
            predicate::str::contains("Promotion status: ready_with_warnings")
                .and(predicate::str::contains(
                    "exactly 50 tests, minimum 45 passed",
                ))
                .and(predicate::str::contains("Observed: 45/50 passed"))
                .and(predicate::str::contains("warning:"))
                .and(predicate::str::contains("Assessment only; inspect status.")),
        );
    let mut approval = promotion_arguments("approve-promotion", "demo", "build", "v1");
    approval.push("--approve");
    fixture
        .command()
        .args(&approval)
        .assert()
        .code(1)
        .stdout("")
        .stderr(predicate::str::contains(
            "error [optional_steps_unacknowledged]",
        ));
    approval.extend(["--acknowledge-optional", "--note", "Reviewed externally"]);
    fixture
        .command()
        .args(&approval)
        .assert()
        .success()
        .stderr("")
        .stdout(
            predicate::str::contains("Promotion-criteria approval recorded:")
                .and(predicate::str::contains(
                    "Skipped optional criteria (acknowledged): personal-optional",
                ))
                .and(predicate::str::contains("Note: Reviewed externally"))
                .and(predicate::str::contains(
                    "This command did not execute commands or deploy files.",
                ))
                .and(predicate::str::contains(
                    "After commands pass, an attached runner can release its completion gate when this approval currently matches its release ref and registration.",
                )),
        );
    fixture
        .command()
        .args(["promotion-approvals", "demo"])
        .assert()
        .success()
        .stderr("")
        .stdout(
            predicate::str::contains("Promotion-criteria approval history (newest first")
                .and(predicate::str::contains("personal-optional"))
                .and(predicate::str::contains("ready_with_warnings")),
        );
    fixture
        .command()
        .args(["harness-results", "demo"])
        .assert()
        .success()
        .stderr("")
        .stdout(
            predicate::str::contains("External harness-result history (newest first")
                .and(predicate::str::contains("Reported: 45/50 passed")),
        );
    fixture.json(&harness_arguments(
        "demo", "build", "smoke", "v1", "49", "49",
    ));
    let blocked = fixture.json(&status_arguments);
    fixture
        .command()
        .args(status_arguments)
        .assert()
        .success()
        .stderr("")
        .stdout(
            predicate::str::contains("Promotion status: blocked")
                .and(predicate::str::contains("Observed: 49/49 passed"))
                .and(predicate::str::contains(
                    "exactly 50 tests, minimum 45 passed",
                ))
                .and(predicate::str::contains(
                    blocked["criteria"][0]["message"].as_str().unwrap(),
                )),
        );
    fixture
        .command()
        .args(["promotion-status", "--help"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("exits 0 even when status is blocked or not_configured")
                .and(predicate::str::contains("callers must inspect status"))
                .and(predicate::str::contains(
                    "approve-promotion as the enforcing gate",
                )),
        );
    fixture.assert_no_execution_or_copy();
}
