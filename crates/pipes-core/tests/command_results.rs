mod support;

use std::fs;

use pipes_core::{CommandResult, ManifestPreview, Registry, preview_manifest};
use rusqlite::{Connection, params};
use serde_json::json;
use support::{FULL, Fixture, MINIMAL, canonical};

fn with_command(fields: &str, prepare: bool) -> String {
    let command = format!("{{id: suite, exec: [arbitrary-tool, '--literal-argv'], {fields}}}");
    if prepare {
        format!("{MINIMAL}prepare: [{command}]\n")
    } else {
        MINIMAL.replace(
            "stages: [{id: build}]",
            &format!("stages: [{{id: build, checks: [{command}]}}]"),
        )
    }
}

fn with_result(result: &str, prepare: bool) -> String {
    with_command(&format!("result: {result}"), prepare)
}

#[test]
fn omitted_rules_default_to_zero_exit_code_without_changing_legacy_summary_json() {
    let fixture = Fixture::new(FULL);
    let preview = preview_manifest(&fixture.manifest).unwrap();
    assert_eq!(preview.manifest_path, canonical(&fixture.manifest));
    assert_eq!(
        preview.prepare[0].result.as_ref(),
        &CommandResult::default()
    );
    assert_eq!(
        preview.stages[0].checks[0].result.as_ref(),
        &CommandResult::default()
    );
    assert_eq!(
        serde_json::to_value(CommandResult::default()).unwrap(),
        json!({"type": "exitCode", "successCodes": [0]})
    );
    let snapshot = serde_json::to_value(&preview).unwrap();
    assert!(snapshot["prepare"][0].get("result").is_none());
    assert!(snapshot["stages"][0]["checks"][0].get("result").is_none());
    assert!(
        snapshot["stages"][0]["checkPlan"][0]["command"]
            .get("result")
            .is_none()
    );
    assert_eq!(
        serde_json::from_value::<ManifestPreview>(snapshot).unwrap(),
        preview
    );
}

#[test]
fn explicit_rules_support_prepare_and_nested_leaves_with_camel_case_summaries() {
    let yaml = MINIMAL
        .replace("source: {path: source}", "source: {path: source, snapshot: working-tree, require_clean: false}")
        .replace(
            "stages: [{id: build}]",
            r#"prepare:
  - id: prepare
    exec: [arbitrary-tool, 'argument with spaces', '']
    result: {type: exit-code, success_codes: [0, 7, 255]}
stages:
  - id: build
    checks:
      - id: batch
        parallel:
          - id: nested
            sequence:
              - id: suite
                exec: [another-tool]
                cwd: generated/reports
                result: {type: json-file, path: status.json, pointer: /status, success: passed, failure: failed}"#,
        );
    let fixture = Fixture::new(&yaml);
    let preview = preview_manifest(&fixture.manifest).unwrap();
    assert_eq!(
        serde_json::to_value(&preview.prepare[0].result).unwrap(),
        json!({"type": "exitCode", "successCodes": [0, 7, 255]})
    );
    let expected = json!({
        "type": "jsonFile", "path": "status.json", "pointer": "/status",
        "success": "passed", "failure": "failed"
    });
    let snapshot = serde_json::to_value(&preview).unwrap();
    assert_eq!(snapshot["stages"][0]["checks"][0]["result"], expected);
    assert_eq!(
        snapshot["stages"][0]["checkPlan"][0]["steps"][0]["steps"][0]["command"]["result"],
        expected
    );
    assert_eq!(
        serde_json::from_value::<ManifestPreview>(snapshot).unwrap(),
        preview
    );
    assert_eq!(preview.manifest_yaml, yaml);
    assert!(!fixture.source.join("generated").exists());
    assert!(!fixture.state.exists());
}

#[test]
fn malformed_null_unknown_and_duplicate_rule_fields_are_rejected_in_both_command_scopes() {
    let fixture = Fixture::new(MINIMAL);
    let json_rule =
        "type: json-file, path: status.json, pointer: /status, success: passed, failure: failed";
    for result in [
        "null".to_owned(),
        "~".to_owned(),
        "[]".to_owned(),
        "exit-code".to_owned(),
        "true".to_owned(),
        "{}".to_owned(),
        "{type: null, success_codes: [0]}".to_owned(),
        "{type: exitCode, success_codes: [0]}".to_owned(),
        "{type: stdout, success_codes: [0]}".to_owned(),
        "{type: exit-code}".to_owned(),
        "{type: exit-code, success_codes: [0], unknown: value}".to_owned(),
        "{type: exit-code, success_codes: [0], path: status.json}".to_owned(),
        "{type: exit-code, type: exit-code, success_codes: [0]}".to_owned(),
        "{type: exit-code, success_codes: [0], success_codes: [0]}".to_owned(),
        format!("{{{json_rule}, success_codes: [0]}}"),
        format!("{{{json_rule}, unknown: value}}"),
        format!("{{{json_rule}, path: other.json}}"),
        format!("{{{json_rule}, pointer: /other}}"),
        format!("{{{json_rule}, success: passed}}"),
        format!("{{{json_rule}, failure: failed}}"),
        format!("{{{json_rule}, type: json-file}}"),
        "{type: exit-code, success_codes: [0]}, result: {type: exit-code, success_codes: [0]}"
            .to_owned(),
    ] {
        for prepare in [true, false] {
            fixture.write(&with_result(&result, prepare));
            let error = preview_manifest(&fixture.manifest).unwrap_err();
            assert_eq!(error.code(), "invalid_manifest", "{result}: {error}");
        }
    }
}

#[test]
fn exit_code_rules_require_nonempty_unique_portable_integers() {
    let fixture = Fixture::new(MINIMAL);
    for codes in [
        "null",
        "[]",
        "[0, 0]",
        "[-1]",
        "[256]",
        "[2147483648]",
        "[1.5]",
        "[true]",
        "['0']",
        "0",
        "'[0]'",
    ] {
        for prepare in [true, false] {
            fixture.write(&with_result(
                &format!("{{type: exit-code, success_codes: {codes}}}"),
                prepare,
            ));
            let error = preview_manifest(&fixture.manifest).unwrap_err();
            assert_eq!(error.code(), "invalid_manifest", "{codes}: {error}");
        }
    }
    fixture.write(&with_result(
        "{type: exit-code, success_codes: [255, 0, 1]}",
        false,
    ));
    assert_eq!(
        preview_manifest(&fixture.manifest).unwrap().stages[0].checks[0]
            .result
            .as_ref(),
        &CommandResult::ExitCode {
            success_codes: vec![255, 0, 1]
        }
    );
}

#[test]
fn json_rules_validate_paths_pointers_and_distinct_non_null_scalars() {
    let fixture = Fixture::new(MINIMAL);
    let valid =
        "{type: json-file, path: status.json, pointer: /status, success: passed, failure: failed}";
    for (from, to) in [
        ("path: status.json", "path: ''"),
        ("path: status.json", "path: '   '"),
        ("path: status.json", r#"path: "\0""#),
        ("path: status.json", "path: null"),
        ("path: status.json", "path: true"),
        ("path: status.json", "path: 123"),
        ("pointer: /status", "pointer: null"),
        ("pointer: /status", "pointer: 123"),
        ("pointer: /status", "pointer: status"),
        ("pointer: /status", "pointer: '#/status'"),
        ("pointer: /status", "pointer: /~"),
        ("pointer: /status", "pointer: /~2"),
        ("pointer: /status", "pointer: /~1~"),
        ("success: passed", "success: null"),
        ("success: passed", "success: []"),
        ("success: passed", "success: {}"),
        ("success: passed", "success: .nan"),
        ("success: passed", "success: .inf"),
        ("failure: failed", "failure: null"),
        ("failure: failed", "failure: []"),
        ("failure: failed", "failure: {}"),
        ("failure: failed", "failure: -.inf"),
        ("failure: failed", "failure: passed"),
        (
            "success: passed, failure: failed",
            "success: true, failure: true",
        ),
        ("success: passed, failure: failed", "success: 7, failure: 7"),
        ("path: status.json, ", ""),
        ("pointer: /status, ", ""),
        ("success: passed, ", ""),
        (", failure: failed", ""),
    ] {
        let result = valid.replace(from, to);
        assert_ne!(result, valid);
        for prepare in [true, false] {
            fixture.write(&with_result(&result, prepare));
            let error = preview_manifest(&fixture.manifest).unwrap_err();
            assert_eq!(error.code(), "invalid_manifest", "{result}: {error}");
        }
    }
}

#[test]
fn json_rules_accept_scalar_types_root_and_escaped_pointers_without_coercion() {
    let fixture = Fixture::new(MINIMAL);
    for (pointer, success, failure) in [
        ("", json!(true), json!(false)),
        ("/", json!(""), json!("failed")),
        ("/a~1b/~0c/0", json!(7), json!(0)),
        ("/~01", json!(1.5), json!(2.5)),
        ("/雪☃", json!(1), json!("1")),
        ("/a%2Fb", json!(true), json!("true")),
        (
            "/\0",
            json!(9007199254740993_u64),
            json!(9007199254740992_u64),
        ),
    ] {
        let result = format!(
            "{{type: json-file, path: '/absolute status.json', pointer: {}, success: {success}, failure: {failure}}}",
            serde_json::to_string(pointer).unwrap()
        );
        for prepare in [true, false] {
            fixture.write(&with_result(&result, prepare));
            let preview = preview_manifest(&fixture.manifest).unwrap();
            let command = if prepare {
                &preview.prepare[0]
            } else {
                &preview.stages[0].checks[0]
            };
            assert_eq!(
                command.result.as_ref(),
                &CommandResult::JsonFile {
                    path: "/absolute status.json".to_owned(),
                    pointer: pointer.to_owned(),
                    success: success.clone(),
                    failure: failure.clone(),
                }
            );
        }
    }
}

#[test]
fn results_are_not_allowed_on_groups_or_stages() {
    let fixture = Fixture::new(MINIMAL);
    for kind in ["sequence", "parallel"] {
        for fields in [
            format!(
                "{kind}: [{{id: suite, exec: [tool]}}], result: {{type: exit-code, success_codes: [0]}}"
            ),
            format!(
                "result: {{type: exit-code, success_codes: [0]}}, {kind}: [{{id: suite, exec: [tool]}}]"
            ),
        ] {
            fixture.write(&MINIMAL.replace(
                "stages: [{id: build}]",
                &format!("stages: [{{id: build, checks: [{{id: group, {fields}}}]}}]"),
            ));
            let error = preview_manifest(&fixture.manifest).unwrap_err();
            assert!(error.to_string().contains("not groups"), "{error}");
        }
    }
    fixture.write(&MINIMAL.replace(
        "stages: [{id: build}]",
        "stages: [{id: build, result: {type: exit-code, success_codes: [0]}}]",
    ));
    assert!(
        preview_manifest(&fixture.manifest)
            .unwrap_err()
            .to_string()
            .contains("unknown field")
    );
}

#[test]
fn env_and_secrets_cannot_override_reserved_runner_variables_in_either_scope() {
    let fixture = Fixture::new(MINIMAL);
    for reserved in [
        "PIPES_RUN_ID",
        "PIPES_RELEASE_REF",
        "PIPES_STAGE_ID",
        "PIPES_COMMAND_ID",
        "PIPES_RESULT_FILE",
    ] {
        for name in [reserved.to_owned(), reserved.to_ascii_lowercase()] {
            for (map, value) in [("env", "attempted-override"), ("secrets", "env://TOKEN")] {
                for prepare in [true, false] {
                    fixture.write(&with_command(
                        &format!("{map}: {{{name}: {value}}}"),
                        prepare,
                    ));
                    let error = preview_manifest(&fixture.manifest).unwrap_err();
                    assert_eq!(error.code(), "invalid_manifest");
                    assert!(
                        error
                            .to_string()
                            .contains("reserved for the pipeline runner"),
                        "{error}"
                    );
                    assert!(error.to_string().contains(&name), "{error}");
                }
            }
        }
    }
    fixture.write(&with_command("secrets: {TOKEN: env://PIPES_RUN_ID}", false));
    assert!(preview_manifest(&fixture.manifest).is_ok());
}

#[test]
fn summary_result_tags_are_strict_camel_case_with_no_null_or_duplicate_fields() {
    for value in [
        json!({"type": "exitCode", "successCodes": [0, 7]}),
        json!({"type": "jsonFile", "path": "result.json", "pointer": "", "success": true, "failure": false}),
    ] {
        let parsed: CommandResult = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), value);
    }
    for value in [
        "null",
        r#"{"type":"exit-code","successCodes":[0]}"#,
        r#"{"type":"exitCode","success_codes":[0]}"#,
        r#"{"type":"exitCode","successCodes":null}"#,
        r#"{"type":"exitCode","successCodes":[0],"extra":true}"#,
        r#"{"type":"exitCode","type":"exitCode","successCodes":[0]}"#,
        r#"{"type":"exitCode","successCodes":[0],"successCodes":[7]}"#,
        r#"{"type":"jsonFile","path":"x","path":"y","pointer":"","success":true,"failure":false}"#,
    ] {
        assert!(
            serde_json::from_str::<CommandResult>(value).is_err(),
            "{value}"
        );
    }
}

fn downgrade(registry: &Registry, fixture: &Fixture, version: i64) -> (Connection, String) {
    let app = registry.get("demo").unwrap();
    let mut snapshot = serde_json::to_value(&app.manifest).unwrap();
    if version < 3 {
        snapshot["execution"]
            .as_object_mut()
            .unwrap()
            .remove("maxParallel");
    }
    for stage in snapshot["stages"].as_array_mut().unwrap() {
        for (introduced, field) in [
            (2, "after"),
            (3, "checkPlan"),
            (4, "consumers"),
            (5, "promotionCriteria"),
        ] {
            if version < introduced {
                stage.as_object_mut().unwrap().remove(field);
            }
        }
    }
    let connection = Connection::open(fixture.state.join("registry.sqlite3")).unwrap();
    connection
        .execute_batch("DROP TABLE pipeline_run_logs; DROP TABLE pipeline_runs;")
        .unwrap();
    if version < 5 {
        connection
            .execute_batch("DROP TABLE harness_results; DROP TABLE promotion_approvals;")
            .unwrap();
    }
    if version < 4 {
        connection
            .execute_batch("DROP TABLE consumer_deployments;")
            .unwrap();
    }
    let snapshot = snapshot.to_string();
    connection
        .execute("UPDATE registered_apps SET snapshot = ?1", [&snapshot])
        .unwrap();
    connection
        .pragma_update(None, "user_version", version)
        .unwrap();
    (connection, snapshot)
}

#[test]
fn schema_v6_results_and_working_tree_opt_in_round_trip_without_original_files() {
    let yaml = with_result(
        "{type: json-file, path: status.json, pointer: /status, success: passed, failure: failed}",
        false,
    )
    .replace(
        "path: source",
        "path: source, snapshot: working-tree, require_clean: false",
    );
    let fixture = Fixture::new(&yaml);
    let registry = Registry::open(&fixture.state).unwrap();
    let registered = registry.register(&fixture.manifest, None, false).unwrap();
    fs::remove_file(&fixture.manifest).unwrap();
    fs::remove_dir(&fixture.source).unwrap();
    let reopened = Registry::open(&fixture.state).unwrap();
    assert_eq!(reopened.get("demo").unwrap(), registered);
    assert_eq!(reopened.list().unwrap(), [registered]);
    assert!(!fixture.path("status.json").exists());
    assert!(fixture.root.path().exists());
}

#[test]
fn schema_v1_through_v5_reject_new_syntax_offline_without_changing_saved_rows() {
    for version in 1..=5 {
        for (yaml, field) in [
            (
                MINIMAL.replace("path: source", "path: source, snapshot: working-tree"),
                "source.snapshot",
            ),
            (
                with_result("{type: exit-code, success_codes: [0]}", true),
                "prepare[0].result",
            ),
            (
                with_result("{type: exit-code, success_codes: [0]}", false),
                "stages[0].checks[0].result",
            ),
            (
                with_result(
                    "{type: json-file, path: result.json, pointer: /status, success: passed, failure: failed}",
                    false,
                ),
                "stages[0].checks[0].result",
            ),
        ] {
            let fixture = Fixture::new(&yaml);
            let registry = Registry::open(&fixture.state).unwrap();
            registry.register(&fixture.manifest, None, false).unwrap();
            let (connection, before) = downgrade(&registry, &fixture, version);
            fs::remove_file(&fixture.manifest).unwrap();
            fs::remove_dir(&fixture.source).unwrap();
            let error = Registry::open(&fixture.state).unwrap_err();
            assert_eq!(error.code(), "storage", "version {version}: {error}");
            assert!(error.to_string().contains(field), "{error}");
            assert!(
                error
                    .to_string()
                    .contains("schema-v1/v2/v3/v4/v5 snapshots"),
                "{error}"
            );
            let actual_version: i64 = connection
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap();
            assert_eq!(actual_version, version);
            let after: String = connection
                .query_row("SELECT snapshot FROM registered_apps", [], |row| row.get(0))
                .unwrap();
            assert_eq!(after, before);
        }
    }
}

#[test]
fn schema_v3_through_v5_reject_results_inside_nested_groups() {
    let yaml = MINIMAL.replace(
        "stages: [{id: build}]",
        "stages: [{id: build, checks: [{id: outer, sequence: [{id: inner, parallel: [{id: suite, exec: [tool], result: {type: exit-code, success_codes: [0]}}]}]}]}]",
    );
    for version in 3..=5 {
        let fixture = Fixture::new(&yaml);
        let registry = Registry::open(&fixture.state).unwrap();
        registry.register(&fixture.manifest, None, false).unwrap();
        let _saved = downgrade(&registry, &fixture, version);
        let error = Registry::open(&fixture.state).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("checks[0].sequence[0].parallel[0].result"),
            "{error}"
        );
    }
}

#[test]
fn old_snapshots_without_new_syntax_keep_default_results_after_offline_migration() {
    for version in 1..=5 {
        let fixture = Fixture::new(&with_command("env: {MODE: test}", false));
        let registry = Registry::open(&fixture.state).unwrap();
        let original = registry.register(&fixture.manifest, None, false).unwrap();
        let (_connection, _) = downgrade(&registry, &fixture, version);
        fs::remove_file(&fixture.manifest).unwrap();
        fs::remove_dir(&fixture.source).unwrap();
        let reopened = Registry::open(&fixture.state).unwrap();
        assert_eq!(reopened.get("demo").unwrap(), original);
    }
}

#[test]
fn current_snapshot_rules_must_match_saved_yaml_in_both_flat_and_planned_checks() {
    let fixture = Fixture::new(&with_result("{type: exit-code, success_codes: [7]}", false));
    let registry = Registry::open(&fixture.state).unwrap();
    let registered = registry.register(&fixture.manifest, None, false).unwrap();
    let mut snapshot = serde_json::to_value(&registered.manifest).unwrap();
    let forged = json!({"type": "exitCode", "successCodes": [0]});
    snapshot["stages"][0]["checks"][0]["result"] = forged.clone();
    snapshot["stages"][0]["checkPlan"][0]["command"]["result"] = forged;
    let connection = Connection::open(fixture.state.join("registry.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE registered_apps SET snapshot = ?1 WHERE app_id = ?2",
            params![snapshot.to_string(), registered.manifest.app_id],
        )
        .unwrap();
    let error = registry.get("demo").unwrap_err();
    assert_eq!(error.code(), "storage");
    assert!(
        error.to_string().contains("do not match the YAML snapshot"),
        "{error}"
    );
}
