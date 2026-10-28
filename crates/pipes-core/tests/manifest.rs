mod support;

use std::fs;

use pipes_core::{ManifestPreview, Registry, preview_manifest};
use serde_json::Value;
use sha2::{Digest, Sha256};
use support::{FULL, Fixture, MINIMAL, canonical};

#[test]
fn minimum_manifest_defaults_display_name_and_optional_fields() {
    let fixture = Fixture::new(MINIMAL);
    let preview = preview_manifest(&fixture.manifest).unwrap();
    assert_eq!(preview.app_id, "demo");
    assert_eq!(preview.name, "demo");
    assert_eq!(preview.manifest_path, canonical(&fixture.manifest));
    assert_eq!(preview.source_path, canonical(&fixture.source));
    assert_eq!(preview.manifest_yaml, MINIMAL);
    assert_eq!(preview.version_from, "file:VERSION");
    assert_eq!(preview.stages.len(), 1);
    assert_eq!(preview.stages[0].id, "build");
    assert!(preview.stages[0].after.is_empty());
    assert!(preview.stages[0].checks.is_empty());
    assert!(preview.stages[0].requires.is_empty());
    assert!(!preview.stages[0].requires_approval);
    assert!(preview.prepare.is_empty());
    assert!(preview.outputs.is_empty());
    let json = serde_json::to_value(&preview).unwrap();
    assert_eq!(json["appId"], "demo");
    assert_eq!(
        json["execution"],
        serde_json::json!({"os": null, "arch": null, "timeout": null, "maxParallel": "auto"})
    );
    assert!(json.get("app_id").is_none());
    assert_eq!(
        serde_json::from_value::<ManifestPreview>(json).unwrap(),
        preview
    );
    assert!(!fixture.state.exists());
}

#[test]
fn full_manifest_preserves_order_and_planned_configuration_without_prerequisites() {
    let fixture = Fixture::new(FULL);
    let preview = preview_manifest(&fixture.manifest).unwrap();
    assert_eq!(preview.name, "Sample Application");
    assert_eq!(preview.execution.os.as_deref(), Some("macos"));
    assert_eq!(preview.execution.arch.as_deref(), Some("arm64"));
    assert_eq!(preview.execution.timeout.as_deref(), Some("1h 30m"));
    assert_eq!(preview.stages[0].id, "tested");
    assert_eq!(preview.stages[1].id, "shipped");
    assert!(preview.stages[0].after.is_empty());
    assert_eq!(preview.stages[1].after, ["tested"]);
    assert_eq!(preview.stages[0].checks[0].exec, ["cargo", "test"]);
    assert_eq!(
        preview.stages[0].requires,
        ["checks.passed", "prepare.version.passed"]
    );
    assert!(preview.stages[1].requires_approval);
    assert_eq!(preview.prepare[0].exec[2], "");
    assert_eq!(
        preview.prepare[0].cwd.as_deref(),
        Some("directory-not-created")
    );
    assert_eq!(preview.prepare[0].timeout.as_deref(), Some("10s"));
    assert_eq!(preview.prepare[0].env["EMPTY"], "");
    assert_eq!(
        preview.prepare[0].secrets["TOKEN"],
        "env://PIPES_TEST_TOKEN"
    );
    assert_eq!(
        preview.prepare[0].secrets["ACCOUNT"],
        "keychain://service account"
    );
    assert_eq!(preview.outputs[0].name, "binary");
    assert_eq!(preview.outputs[1].name, "bundle");
    assert_eq!(preview.manifest_yaml, FULL);
    assert!(!fixture.source.join("VERSION").exists());
    assert!(!fixture.source.join(".git").exists());
    assert!(!fixture.state.exists());
}

#[test]
fn hashes_exact_bytes_including_line_endings_and_comments() {
    let fixture = Fixture::new(MINIMAL);
    let first = preview_manifest(&fixture.manifest).unwrap();
    assert_eq!(
        first.manifest_hash,
        format!("{:x}", Sha256::digest(MINIMAL.as_bytes()))
    );
    let crlf = MINIMAL.replace('\n', "\r\n");
    fixture.write(&crlf);
    let second = preview_manifest(&fixture.manifest).unwrap();
    assert_eq!(second.manifest_yaml, crlf);
    assert_eq!(second.manifest_hash.len(), 64);
    assert_ne!(first.manifest_hash, second.manifest_hash);
    fixture.write(&format!("{MINIMAL}# comment\n"));
    assert_ne!(
        first.manifest_hash,
        preview_manifest(&fixture.manifest).unwrap().manifest_hash
    );
}

#[test]
fn unicode_and_space_paths_are_canonical_and_absolute() {
    let fixture = Fixture::new(MINIMAL);
    let source = fixture.path("source 日本 café");
    fs::rename(&fixture.source, &source).unwrap();
    let manifest = fixture.path("Release Plan 日本.yaml");
    fs::rename(&fixture.manifest, &manifest).unwrap();
    let yaml = MINIMAL.replace("path: source", "path: 'source 日本 café'");
    fs::write(&manifest, &yaml).unwrap();
    let preview = preview_manifest(&manifest).unwrap();
    assert_eq!(preview.manifest_path, canonical(&manifest));
    assert_eq!(preview.source_path, canonical(&source));
    fs::write(
        &manifest,
        yaml.replace(
            "'source 日本 café'",
            &serde_json::to_string(&canonical(&source)).unwrap(),
        ),
    )
    .unwrap();
    assert_eq!(
        preview_manifest(&manifest).unwrap().source_path,
        canonical(&source)
    );
}

#[cfg(unix)]
#[test]
fn symlinked_manifest_uses_the_canonical_manifest_parent() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new(MINIMAL);
    let elsewhere = fixture.path("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    let link = elsewhere.join("linked.yaml");
    symlink(&fixture.manifest, &link).unwrap();
    let preview = preview_manifest(&link).unwrap();
    assert_eq!(preview.manifest_path, canonical(&fixture.manifest));
    assert_eq!(preview.source_path, canonical(&fixture.source));
}

#[test]
fn rejects_unknown_fields_at_every_schema_level() {
    let fixture = Fixture::new(FULL);
    let changes = [
        ("api: pipes/v1", "api: pipes/v1\nunknown: true"),
        (
            "  snapshot: git-commit",
            "  unknown: true\n  snapshot: git-commit",
        ),
        ("  scheme: semver", "  unknown: true\n  scheme: semver"),
        ("execution:\n", "execution:\n  unknown: true\n"),
        ("    os: macos", "    unknown: true\n    os: macos"),
        (
            "    cwd: directory-not-created",
            "    unknown: true\n    cwd: directory-not-created",
        ),
        (
            "    path: dist/My App.app",
            "    unknown: true\n    path: dist/My App.app",
        ),
        ("  - id: tested", "  - id: tested\n    unknown: true"),
        (
            "        exec: [cargo, test]",
            "        unknown: true\n        exec: [cargo, test]",
        ),
        ("  mode: keep-all", "  unknown: true\n  mode: keep-all"),
    ];
    for (from, to) in changes {
        fixture.write(&FULL.replacen(from, to, 1));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest", "{from}");
        assert!(error.to_string().contains("unknown"), "{error}");
        assert!(error.to_string().contains("line"), "{error}");
    }
}

#[test]
fn rejects_malformed_yaml_and_non_string_scalars_with_locations() {
    let fixture = Fixture::new(MINIMAL);
    let cases = [
        MINIMAL.replace("[{id: build}]", "[{id: build}"),
        MINIMAL.replace("app: demo", "app: 123"),
        MINIMAL.replace("app: demo", "app: true"),
        MINIMAL.replace("path: source", "path: false"),
        MINIMAL.replace("from: 'file:VERSION'", "from: 123"),
        format!("{MINIMAL}name: 42\n"),
        format!("{MINIMAL}prepare: [{{id: build, exec: [123]}}]\n"),
        format!("{MINIMAL}prepare: [{{id: build, exec: [tool], env: {{VALUE: true}}}}]\n"),
        format!("{MINIMAL}outputs: {{123: {{path: out, format: file}}}}\n"),
    ];
    for yaml in cases {
        fixture.write(&yaml);
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest", "{yaml}");
        assert!(error.to_string().contains("line"), "{error}");
    }
    fixture.write(&format!("{MINIMAL}---\n{MINIMAL}"));
    let error = preview_manifest(&fixture.manifest).unwrap_err();
    assert_eq!(error.code(), "invalid_manifest");
    assert!(error.to_string().contains("more than one document"));
}

#[test]
fn rejects_unknown_values_and_empty_required_fields() {
    let fixture = Fixture::new(FULL);
    let cases = [
        ("pipes/v1", "pipes/v2", "api"),
        ("app: sample-app", "app: Sample_App", "app"),
        ("name: Sample Application", "name: ''", "name"),
        ("path: source", "path: ''", "source.path"),
        (
            "snapshot: git-commit",
            "snapshot: archive",
            "source.snapshot",
        ),
        ("scheme: semver", "scheme: calendar", "version.scheme"),
        ("from: file:VERSION", "from: ''", "version.from"),
        ("os: macos", "os: android", "execution.host.os"),
        ("arch: arm64", "arch: arm", "execution.host.arch"),
        ("path: dist/My App.app", "path: ''", "outputs.bundle.path"),
        ("format: directory", "format: zip", "outputs.bundle.format"),
        ("bundle:\n", "Bad_Name:\n", "outputs.Bad_Name"),
        ("mode: keep-all", "mode: purge", "retention.mode"),
        (
            "stage_heads, pins, active_runs",
            "stage_heads, unknown",
            "retention.protect",
        ),
        (
            "stage_heads, pins, active_runs",
            "pins, pins",
            "retention.protect",
        ),
    ];
    for (from, to, field) in cases {
        fixture.write(&FULL.replacen(from, to, 1));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest", "{from}");
        assert!(error.to_string().contains(field), "{field}: {error}");
    }
    fixture.write(&MINIMAL.replace("[{id: build}]", "[]"));
    assert!(
        preview_manifest(&fixture.manifest)
            .unwrap_err()
            .to_string()
            .contains("stages")
    );
}

#[test]
fn rejects_duplicate_stage_and_scoped_command_ids_but_allows_separate_scopes() {
    let fixture = Fixture::new(MINIMAL);
    let cases = [
        MINIMAL.replace("[{id: build}]", "[{id: build}, {id: build}]"),
        format!("{MINIMAL}prepare: [{{id: same, exec: [tool]}}, {{id: same, exec: [tool]}}]\n"),
        MINIMAL.replace(
            "[{id: build}]",
            "[{id: build, checks: [{id: same, exec: [tool]}, {id: same, exec: [tool]}]}]",
        ),
    ];
    for yaml in cases {
        fixture.write(&yaml);
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest");
        assert!(error.to_string().contains("duplicate"));
    }
    fixture.write(&format!(
        "{}prepare: [{{id: check, exec: [tool]}}]\n",
        MINIMAL.replace(
            "[{id: build}]",
            "[{id: build, checks: [{id: check, exec: [tool]}]}, \
             {id: release, checks: [{id: check, exec: [tool]}]}]"
        )
    ));
    assert!(preview_manifest(&fixture.manifest).is_ok());
}

#[test]
fn checks_conditions_against_their_stage_and_prepare_steps() {
    let fixture = Fixture::new(MINIMAL);
    for condition in [
        "checks.passed",
        "previous.passed",
        "approval.remote",
        "prepare.missing.passed",
        "prepare..passed",
        "prepare.a.failed",
        "prepare.a.passed.extra",
        "true",
        "",
    ] {
        let condition = serde_json::to_string(condition).unwrap();
        fixture.write(&MINIMAL.replace(
            "[{id: build}]",
            &format!("[{{id: build, require: [{condition}]}}]"),
        ));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest");
        assert!(
            error.to_string().contains("stages[0].require[0]"),
            "{error}"
        );
    }
}

#[test]
fn validates_commands_environment_secrets_and_durations() {
    let fixture = Fixture::new(MINIMAL);
    let cases = [
        ("id: Bad, exec: [tool]", ".id"),
        ("id: check, exec: []", ".exec"),
        ("id: check, exec: ['']", ".exec[0]"),
        ("id: check, exec: ['   ']", ".exec[0]"),
        ("id: check, exec: [tool], cwd: ''", ".cwd"),
        ("id: check, exec: [tool], timeout: 0s", ".timeout"),
        ("id: check, exec: [tool], timeout: '-1s'", ".timeout"),
        ("id: check, exec: [tool], timeout: eventually", ".timeout"),
        ("id: check, exec: [tool], env: {BAD-NAME: value}", ".env."),
        ("id: check, exec: [tool], env: {'2BAD': value}", ".env."),
        (
            "id: check, exec: [tool], secrets: {TOKEN: placeholder-literal}",
            ".secrets.",
        ),
        (
            "id: check, exec: [tool], secrets: {TOKEN: 'env://'}",
            ".secrets.",
        ),
        (
            "id: check, exec: [tool], secrets: {TOKEN: 'env://BAD-NAME'}",
            ".secrets.",
        ),
        (
            "id: check, exec: [tool], secrets: {TOKEN: 'keychain://'}",
            ".secrets.",
        ),
        (
            "id: check, exec: [tool], secrets: {TOKEN: 'keychain://   '}",
            ".secrets.",
        ),
        (
            "id: check, exec: [tool], secrets: {BAD-NAME: 'env://TOKEN'}",
            ".secrets.",
        ),
        (
            "id: check, exec: [tool], env: {TOKEN: value}, secrets: {TOKEN: 'env://TOKEN'}",
            ".secrets.",
        ),
    ];
    for (command, field) in cases {
        fixture.write(&format!("{MINIMAL}prepare: [{{{command}}}]\n"));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest", "{command}");
        assert!(error.to_string().contains(field), "{error}");
    }
    for duration in [
        "0s",
        "-1s",
        "later",
        "100000000000000000000000000000000000000d",
    ] {
        fixture.write(&format!("{MINIMAL}execution: {{timeout: '{duration}'}}\n"));
        assert!(
            preview_manifest(&fixture.manifest)
                .unwrap_err()
                .to_string()
                .contains("execution.timeout")
        );
    }
}

#[test]
fn rejects_duplicate_yaml_map_keys_and_nul_characters() {
    let fixture = Fixture::new(MINIMAL);
    for yaml in [
        format!("{MINIMAL}prepare: [{{id: check, exec: [tool], env: {{TOKEN: a, TOKEN: b}}}}]\n"),
        format!(
            "{MINIMAL}prepare: [{{id: check, exec: [tool], secrets: {{TOKEN: 'env://A', TOKEN: 'env://B'}}}}]\n"
        ),
        format!(
            "{MINIMAL}outputs: {{out: {{path: a, format: file}}, out: {{path: b, format: file}}}}\n"
        ),
        MINIMAL.replace("app: demo", "app: demo\napp: other"),
    ] {
        fixture.write(&yaml);
        assert!(
            preview_manifest(&fixture.manifest)
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
    }
    for (from, to) in [
        ("path: source", r#"path: "source\0x""#),
        ("from: 'file:VERSION'", r#"from: "file:\0VERSION""#),
    ] {
        fixture.write(&MINIMAL.replace(from, to));
        assert!(
            preview_manifest(&fixture.manifest)
                .unwrap_err()
                .to_string()
                .contains("NUL")
        );
    }
    for extra in [
        r#"name: "a\0b""#,
        r#"prepare: [{id: check, exec: ["a\0b"]}]"#,
        r#"prepare: [{id: check, exec: [tool, "a\0b"]}]"#,
        r#"prepare: [{id: check, exec: [tool], cwd: "a\0b"}]"#,
        r#"prepare: [{id: check, exec: [tool], env: {VALUE: "a\0b"}}]"#,
        r#"prepare: [{id: check, exec: [tool], secrets: {VALUE: "keychain://a\0b"}}]"#,
        r#"outputs: {out: {path: "a\0b", format: file}}"#,
    ] {
        fixture.write(&format!("{MINIMAL}{extra}\n"));
        assert!(
            preview_manifest(&fixture.manifest)
                .unwrap_err()
                .to_string()
                .contains("NUL")
        );
    }
}

#[test]
fn missing_source_and_non_directory_source_are_explicit_errors() {
    let fixture = Fixture::new(MINIMAL);
    fs::remove_dir(&fixture.source).unwrap();
    let error = preview_manifest(&fixture.manifest).unwrap_err();
    assert_eq!(error.code(), "source_not_found");
    assert!(error.to_string().contains("source.path"));
    fs::write(&fixture.source, "not a directory").unwrap();
    let error = preview_manifest(&fixture.manifest).unwrap_err();
    assert_eq!(error.code(), "invalid_manifest");
    assert!(error.to_string().contains("source.path"));
}

#[test]
fn missing_non_regular_and_non_utf8_manifests_are_errors() {
    let fixture = Fixture::new(MINIMAL);
    assert_eq!(
        preview_manifest(fixture.path("missing.yaml"))
            .unwrap_err()
            .code(),
        "manifest_io"
    );
    assert!(
        preview_manifest(&fixture.source)
            .unwrap_err()
            .to_string()
            .contains("regular file")
    );
    fs::write(&fixture.manifest, [0xff, 0xfe]).unwrap();
    assert!(
        preview_manifest(&fixture.manifest)
            .unwrap_err()
            .to_string()
            .contains("UTF-8")
    );
}

#[test]
fn manifest_read_is_limited_to_one_mib() {
    let fixture = Fixture::new(MINIMAL);
    let mut yaml = MINIMAL.to_owned();
    yaml.push('#');
    yaml.push_str(&"x".repeat(1024 * 1024 - yaml.len()));
    fixture.write(&yaml);
    assert_eq!(
        preview_manifest(&fixture.manifest)
            .unwrap()
            .manifest_yaml
            .len(),
        1024 * 1024
    );
    yaml.push('x');
    fixture.write(&yaml);
    let error = preview_manifest(&fixture.manifest).unwrap_err();
    assert_eq!(error.code(), "invalid_manifest");
    assert!(error.to_string().contains("1 MiB"));
}

#[cfg(unix)]
#[test]
fn unreadable_manifests_report_io_errors_when_permissions_are_enforced() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new(MINIMAL);
    fs::set_permissions(&fixture.manifest, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&fixture.manifest).is_err() {
        assert_eq!(
            preview_manifest(&fixture.manifest).unwrap_err().code(),
            "manifest_io"
        );
    }
    fs::set_permissions(&fixture.manifest, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn manifest_commands_are_never_executed_during_any_operation() {
    let fixture = Fixture::new(MINIMAL);
    let marker = fixture.path("MUST-NOT-EXIST");
    let script = format!("touch '{}'", marker.display());
    let exec = serde_json::to_string(&["sh", "-c", script.as_str()]).unwrap();
    fixture.write(&format!(
        "{MINIMAL}prepare: [{{id: sentinel, exec: {exec}}}]\n"
    ));
    let preview = preview_manifest(&fixture.manifest).unwrap();
    let registry = Registry::open(&fixture.state).unwrap();
    registry
        .register(&fixture.manifest, Some(&preview.manifest_hash), false)
        .unwrap();
    registry.list().unwrap();
    let registered = registry.get("demo").unwrap();
    let json: Value = serde_json::to_value(registered).unwrap();
    assert_eq!(json["manifest"]["prepare"][0]["exec"][2], script);
    registry.unregister("demo").unwrap();
    assert!(!marker.exists());
}

fn graph_yaml(stages: &str) -> String {
    MINIMAL.replace("stages: [{id: build}]", &format!("stages: {stages}"))
}

#[test]
fn fork_join_graph_preserves_declared_stage_and_parent_order() {
    let yaml = graph_yaml(
        r#"[
  {id: build},
  {id: test, after: [build], checks: [{id: unit, exec: [cargo, test]}], require: [checks.passed, previous.passed]},
  {id: lint, after: [build]},
  {id: package, after: [lint, test], require: [previous.passed]},
  {id: release, require: [previous.passed, approval.local]}
]"#,
    );
    let fixture = Fixture::new(&yaml);
    let preview = preview_manifest(&fixture.manifest).unwrap();
    let ids: Vec<_> = preview
        .stages
        .iter()
        .map(|stage| stage.id.as_str())
        .collect();
    assert_eq!(ids, ["build", "test", "lint", "package", "release"]);
    assert!(preview.stages[0].after.is_empty());
    assert_eq!(preview.stages[1].after, ["build"]);
    assert_eq!(preview.stages[2].after, ["build"]);
    assert_eq!(preview.stages[3].after, ["lint", "test"]);
    assert_eq!(preview.stages[3].requires, ["previous.passed"]);
    assert_eq!(preview.stages[4].after, ["package"]);
    assert!(preview.stages[4].requires_approval);
    assert_eq!(preview.manifest_yaml, yaml);
    let json = serde_json::to_value(&preview).unwrap();
    assert_eq!(
        json["stages"][3]["after"],
        serde_json::json!(["lint", "test"])
    );
    assert_eq!(
        serde_json::from_value::<ManifestPreview>(json).unwrap(),
        preview
    );
}

#[test]
fn independent_roots_and_branches_need_not_rejoin() {
    let fixture = Fixture::new(&graph_yaml(
        "[{id: build}, {id: linux, after: [build]}, {id: windows, after: [build]}, \
          {id: docs, after: []}, {id: docs-published}]",
    ));
    let preview = preview_manifest(&fixture.manifest).unwrap();
    assert!(preview.stages[0].after.is_empty());
    assert_eq!(preview.stages[1].after, ["build"]);
    assert_eq!(preview.stages[2].after, ["build"]);
    assert!(preview.stages[3].after.is_empty());
    assert_eq!(preview.stages[4].after, ["docs"]);
    let ends: Vec<_> = preview
        .stages
        .iter()
        .filter(|stage| {
            !preview
                .stages
                .iter()
                .any(|child| child.after.contains(&stage.id))
        })
        .map(|stage| stage.id.as_str())
        .collect();
    assert_eq!(ends, ["linux", "windows", "docs-published"]);
}

#[test]
fn forward_references_allow_previous_passed_on_the_first_declared_stage() {
    let fixture = Fixture::new(&graph_yaml(
        "[{id: release, after: [tested, signed], require: [previous.passed]}, \
          {id: build, after: []}, {id: signed, after: [build]}, {id: tested, after: [build]}]",
    ));
    let preview = preview_manifest(&fixture.manifest).unwrap();
    let ids: Vec<_> = preview
        .stages
        .iter()
        .map(|stage| stage.id.as_str())
        .collect();
    assert_eq!(ids, ["release", "build", "signed", "tested"]);
    assert_eq!(preview.stages[0].after, ["tested", "signed"]);
    assert_eq!(preview.stages[0].requires, ["previous.passed"]);
    assert!(preview.stages[1].after.is_empty());
}

#[test]
fn previous_passed_is_invalid_on_every_resolved_root() {
    let fixture = Fixture::new(MINIMAL);
    for stages in [
        "[{id: build, after: [], require: [previous.passed]}]",
        "[{id: build}, {id: independent, after: [], require: [previous.passed]}]",
    ] {
        fixture.write(&graph_yaml(stages));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest");
        assert!(error.to_string().contains("require[0]"), "{error}");
        assert!(
            error.to_string().contains("resolved after predecessor"),
            "{error}"
        );
    }
}

#[test]
fn graph_references_reject_unknown_duplicate_self_and_invalid_identifiers() {
    let fixture = Fixture::new(MINIMAL);
    for (after, reason) in [
        ("[missing]", "unknown predecessor"),
        ("[root, root]", "duplicate predecessor"),
        ("[child]", "cannot depend on itself"),
        ("[Root]", "slug"),
        ("['']", "slug"),
        ("[bad_name]", "slug"),
        ("[-root]", "slug"),
        ("[root--child]", "slug"),
        ("['日本']", "slug"),
        (r#"["root\0"]"#, "slug"),
    ] {
        fixture.write(&graph_yaml(&format!(
            "[{{id: root}}, {{id: child, after: {after}}}]"
        )));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest", "{after}");
        assert!(error.to_string().contains("stages[1].after["), "{error}");
        assert!(error.to_string().contains(reason), "{error}");
    }
}

#[test]
fn explicit_after_must_be_a_non_null_array_of_strings() {
    let fixture = Fixture::new(MINIMAL);
    for after in [
        "null",
        "~",
        "",
        "root",
        "42",
        "true",
        "{}",
        "[null]",
        "[42]",
        "[true]",
        "[[]]",
        "[{}]",
        "{all: [root]}",
        "{any: [root]}",
    ] {
        fixture.write(&graph_yaml(&format!(
            "[{{id: root}}, {{id: child, after: {after}}}]"
        )));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest", "{after}");
        assert!(error.to_string().contains("after"), "{error}");
        assert!(error.to_string().contains("line"), "{error}");
    }
    fixture.write(&graph_yaml(
        "[{id: root}, {id: child, after: [root], join: any}]",
    ));
    assert!(
        preview_manifest(&fixture.manifest)
            .unwrap_err()
            .to_string()
            .contains("unknown field")
    );
}

#[test]
fn cycles_are_rejected_including_implicit_edges_and_disconnected_components() {
    let fixture = Fixture::new(MINIMAL);
    for stages in [
        "[{id: a, after: [b]}, {id: b, after: [a]}]",
        "[{id: a, after: [b]}, {id: b, after: [c]}, {id: c, after: [a]}]",
        "[{id: a, after: [b]}, {id: b}]",
        "[{id: root}, {id: a, after: [b]}, {id: b, after: [a]}]",
        "[{id: root}, {id: a, after: [b]}, {id: b}]",
    ] {
        let yaml = graph_yaml(stages);
        fixture.write(&yaml);
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest");
        assert!(error.to_string().contains(".after"), "{error}");
        assert!(error.to_string().contains("cycle"), "{error}");
        assert_eq!(fs::read_to_string(&fixture.manifest).unwrap(), yaml);
        assert!(!fixture.state.exists());
    }
}

#[test]
fn public_stage_dtos_require_after_instead_of_silently_defaulting_it() {
    let fixture = Fixture::new(MINIMAL);
    let preview = preview_manifest(&fixture.manifest).unwrap();
    let mut json = serde_json::to_value(&preview).unwrap();
    json["stages"][0].as_object_mut().unwrap().remove("after");
    let error = serde_json::from_value::<ManifestPreview>(json.clone()).unwrap_err();
    assert!(error.to_string().contains("missing field `after`"));
    json["stages"][0]["after"] = Value::Null;
    assert!(serde_json::from_value::<ManifestPreview>(json).is_err());
}

#[test]
fn deep_graph_validation_is_iterative_and_still_rejects_a_closing_cycle() {
    let mut stages = String::new();
    for index in 0..5000 {
        stages.push_str(&format!("\n  - {{id: s-{index}}}"));
    }
    let yaml = graph_yaml(&stages);
    assert!(yaml.len() < 1024 * 1024);
    let fixture = Fixture::new(&yaml);
    let preview = preview_manifest(&fixture.manifest).unwrap();
    assert_eq!(preview.stages.len(), 5000);
    assert!(preview.stages[0].after.is_empty());
    assert_eq!(preview.stages[4999].after, ["s-4998"]);
    fixture.write(&yaml.replacen("{id: s-0}", "{id: s-0, after: [s-4999]}", 1));
    assert!(
        preview_manifest(&fixture.manifest)
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
}
