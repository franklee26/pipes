mod support;

use pipes_core::{
    ConsumerApproval, ConsumerSummary, ManifestPreview, RecordConsumerDeployment, Registry,
    preview_manifest,
};
use rusqlite::Connection;
use serde_json::{Value, json};
use support::{FULL, Fixture, MINIMAL, canonical};

fn with_consumers(consumers: &str) -> String {
    MINIMAL.replace(
        "[{id: build}]",
        &format!("[{{id: build, consumers: {consumers}}}]"),
    )
}

#[test]
fn omitted_consumers_are_empty_and_optional_consumer_fields_default_only_in_yaml() {
    let fixture = Fixture::new(MINIMAL);
    let preview = preview_manifest(&fixture.manifest).unwrap();
    assert!(preview.stages[0].consumers.is_empty());
    assert_eq!(
        serde_json::to_value(&preview).unwrap()["stages"][0]["consumers"],
        json!([])
    );
    fixture.write(&with_consumers("[]"));
    assert!(
        preview_manifest(&fixture.manifest).unwrap().stages[0]
            .consumers
            .is_empty()
    );

    fixture.write(&with_consumers(
        r#"[
            {id: personal-mac, target: "MacBook / Applications"},
            {id: office, name: "Office laptop", target: "not-an-existing-device", approval: manual}
        ]"#,
    ));
    let preview = preview_manifest(&fixture.manifest).unwrap();
    assert_eq!(preview.manifest_path, canonical(&fixture.manifest));
    assert_eq!(
        preview.stages[0].consumers,
        [
            ConsumerSummary {
                id: "personal-mac".to_owned(),
                name: "personal-mac".to_owned(),
                target: "MacBook / Applications".to_owned(),
                approval: ConsumerApproval::Manual,
            },
            ConsumerSummary {
                id: "office".to_owned(),
                name: "Office laptop".to_owned(),
                target: "not-an-existing-device".to_owned(),
                approval: ConsumerApproval::Manual,
            },
        ]
    );
    assert_eq!(ConsumerApproval::default(), ConsumerApproval::Manual);
    assert_eq!(
        serde_json::to_value(ConsumerApproval::Manual).unwrap(),
        json!("manual")
    );
    assert!(!fixture.state.exists());
    assert!(!fixture.source.join("VERSION").exists());
    assert!(!fixture.path("not-an-existing-device").exists());
}

#[test]
fn consumer_ids_are_unique_per_stage_not_globally_and_never_become_graph_nodes() {
    let yaml = MINIMAL.replace(
        "[{id: build}]",
        r#"[
            {id: beta, after: [], consumers: [{id: same, target: device-a}, {id: beta, target: self-name}]},
            {id: stable, consumers: [{id: same, target: device-b}]}
        ]"#,
    );
    let fixture = Fixture::new(&yaml);
    let preview = preview_manifest(&fixture.manifest).unwrap();
    assert_eq!(preview.stages.len(), 2);
    assert!(preview.stages[0].after.is_empty());
    assert_eq!(preview.stages[1].after, ["beta"]);
    assert_eq!(
        preview.stages[0].consumers[0].id,
        preview.stages[1].consumers[0].id
    );
    assert_ne!(
        preview.stages[0].consumers[0].target,
        preview.stages[1].consumers[0].target
    );
    assert!(!preview.stages[0].requires_approval);

    fixture.write(&with_consumers(
        "[{id: same, target: a}, {id: same, target: b}]",
    ));
    let error = preview_manifest(&fixture.manifest).unwrap_err();
    assert!(
        error.to_string().contains("duplicate consumer ID"),
        "{error}"
    );
    fixture.write(&yaml.replace(
        "{id: stable, consumers:",
        "{id: stable, after: [same], consumers:",
    ));
    let error = preview_manifest(&fixture.manifest).unwrap_err();
    assert!(
        error.to_string().contains("unknown predecessor 'same'"),
        "{error}"
    );
}

#[test]
fn consumer_metadata_does_not_change_checks_budgets_dependencies_or_approvals() {
    let yaml = FULL
        .replace("execution:\n", "execution:\n  max_parallel: 4\n")
        .replace(
            "    checks:\n      - id: unit\n        exec: [cargo, test]",
            "    checks:\n      - id: group\n        max_parallel: 2\n        parallel:\n          - id: ordered\n            sequence:\n              - id: unit\n                exec: [cargo, test]",
        );
    let fixture = Fixture::new(&yaml);
    let original = preview_manifest(&fixture.manifest).unwrap();
    fixture.write(&yaml.replace(
        "  - id: tested\n",
        "  - id: tested\n    consumers: [{id: shipped, target: display-only}, {id: unit, target: nowhere}]\n",
    ));
    let mut with_consumers = preview_manifest(&fixture.manifest).unwrap();
    assert_eq!(with_consumers.execution, original.execution);
    assert_eq!(with_consumers.stages.len(), original.stages.len());
    assert_eq!(with_consumers.stages[0].consumers.len(), 2);
    with_consumers.stages[0].consumers.clear();
    assert_eq!(with_consumers.stages, original.stages);
    assert_eq!(with_consumers.prepare, original.prepare);
    assert_eq!(with_consumers.outputs, original.outputs);
}

#[test]
fn rejects_null_wrong_types_unknown_fields_and_non_manual_policies() {
    let fixture = Fixture::new(MINIMAL);
    for consumers in [
        "null",
        "~",
        "",
        "false",
        "0",
        "personal-mac",
        "{}",
        "[null]",
        "[[]]",
        "[{target: device}]",
        "[{id: device}]",
        "[{id: null, target: device}]",
        "[{id: true, target: device}]",
        "[{id: 42, target: device}]",
        "[{id: device, target: null}]",
        "[{id: device, target: 42}]",
        "[{id: device, target: true}]",
        "[{id: device, target: []}]",
        "[{id: device, target: {}}]",
        "[{id: device, target: device, name: null}]",
        "[{id: device, target: device, name: 42}]",
        "[{id: device, target: device, name: true}]",
        "[{id: device, target: device, approval: null}]",
        "[{id: device, target: device, approval: automatic}]",
        "[{id: device, target: device, approval: auto}]",
        "[{id: device, target: device, approval: Manual}]",
        "[{id: device, target: device, approval: true}]",
        "[{id: device, target: device, approval: 1}]",
        "[{id: device, target: device, approval: []}]",
        "[{id: device, target: device, approval: {manual: null}}]",
        "[{id: device, target: device, approval: !manual null}]",
        "[{id: device, target: device, unknown: true}]",
        "[{id: device, target: device, target: other}]",
        "[{id: device, target: device, approval: manual, approval: manual}]",
    ] {
        fixture.write(&with_consumers(consumers));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest", "{consumers}: {error}");
    }
    fixture.write(&MINIMAL.replace("{id: build}", "{id: build, consumers: [], consumers: []}"));
    assert_eq!(
        preview_manifest(&fixture.manifest).unwrap_err().code(),
        "invalid_manifest"
    );
}

#[test]
fn consumer_identifiers_and_display_strings_are_strict_and_unicode_bounded() {
    let fixture = Fixture::new(MINIMAL);
    for id in [
        "",
        "-mac",
        "mac-",
        "mac--book",
        "Mac",
        "my_mac",
        "../mac",
        "mac\n",
        "\u{e9}",
    ] {
        let consumers = json!([{"id": id, "target": "device"}]).to_string();
        fixture.write(&with_consumers(&consumers));
        assert_eq!(
            preview_manifest(&fixture.manifest).unwrap_err().code(),
            "invalid_manifest",
            "{id:?}"
        );
    }
    for field in ["name", "target"] {
        for value in [
            "",
            "  ",
            "\t",
            "Mac\nBook",
            "Mac\rBook",
            "a\0b",
            "a\u{7f}b",
            "a\u{85}b",
        ] {
            let mut consumer = json!({"id": "device", "name": "Name", "target": "Target"});
            consumer[field] = json!(value);
            let consumers = json!([consumer])
                .to_string()
                .replace('\u{7f}', "\\u007f")
                .replace('\u{85}', "\\u0085");
            fixture.write(&with_consumers(&consumers));
            let error = preview_manifest(&fixture.manifest).unwrap_err();
            assert_eq!(
                error.code(),
                "invalid_manifest",
                "{field}={value:?}: {error}"
            );
            assert!(error.to_string().contains(field), "{error}");
        }
    }
    for (field, limit) in [("name", 128), ("target", 1024)] {
        for character in ["x", "\u{e9}", "\u{1f4bb}"] {
            let mut consumer = json!({"id": "device", "name": "Name", "target": "Target"});
            consumer[field] = json!(character.repeat(limit));
            fixture.write(&with_consumers(&json!([consumer.clone()]).to_string()));
            preview_manifest(&fixture.manifest).unwrap();
            consumer[field] = json!(character.repeat(limit + 1));
            fixture.write(&with_consumers(&json!([consumer]).to_string()));
            assert_eq!(
                preview_manifest(&fixture.manifest).unwrap_err().code(),
                "invalid_manifest"
            );
        }
    }
    fixture.write(&with_consumers(
        &json!([{"id": "x".repeat(129), "target": "device"}]).to_string(),
    ));
    assert!(
        preview_manifest(&fixture.manifest)
            .unwrap_err()
            .to_string()
            .contains("name")
    );
}

#[test]
fn consumer_alias_expansion_still_obeys_the_manifest_scalar_budget() {
    let destination = "x".repeat(1024);
    let mut consumers = format!("[{{id: first, target: &target '{destination}'}}");
    for index in 0..8500 {
        consumers.push_str(&format!(", {{id: device-{index}, target: *target}}"));
    }
    consumers.push(']');
    let yaml = with_consumers(&consumers);
    assert!(yaml.len() < 1024 * 1024);
    let fixture = Fixture::new(&yaml);
    let error = preview_manifest(&fixture.manifest).unwrap_err();
    assert_eq!(error.code(), "invalid_manifest");
    assert!(
        error.to_string().contains("budget") || error.to_string().contains("repetition"),
        "{error}"
    );
}

#[test]
fn current_json_requires_all_consumer_fields_and_rejects_unknown_fields() {
    let fixture = Fixture::new(&with_consumers("[{id: mac, target: Applications}]"));
    let preview = preview_manifest(&fixture.manifest).unwrap();
    let original = serde_json::to_value(&preview).unwrap();
    assert_eq!(
        serde_json::from_value::<ManifestPreview>(original.clone()).unwrap(),
        preview
    );
    for field in ["id", "name", "target", "approval"] {
        let mut consumer = original["stages"][0]["consumers"][0].clone();
        consumer.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<ConsumerSummary>(consumer).is_err(),
            "{field}"
        );
    }
    for approval in [
        Value::Null,
        json!("auto"),
        json!("automatic"),
        json!(true),
        json!({}),
        json!({"manual": null}),
    ] {
        assert!(serde_json::from_value::<ConsumerApproval>(approval).is_err());
    }
    for replacement in [Value::Null, json!({})] {
        let mut missing = original.clone();
        missing["stages"][0]["consumers"] = replacement;
        assert!(serde_json::from_value::<ManifestPreview>(missing).is_err());
    }
    let mut missing = original.clone();
    missing["stages"][0]
        .as_object_mut()
        .unwrap()
        .remove("consumers");
    assert!(serde_json::from_value::<ManifestPreview>(missing).is_err());
    let mut unknown = original;
    unknown["stages"][0]["consumers"][0]["command"] = json!("deploy");
    assert!(serde_json::from_value::<ManifestPreview>(unknown).is_err());
}

#[test]
fn current_snapshots_reject_missing_or_forged_consumer_metadata_without_writing() {
    let fixture = Fixture::new(&with_consumers(
        "[{id: mac, name: My Mac, target: Applications}]",
    ));
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    let original = serde_json::to_value(&app.manifest).unwrap();
    let connection = Connection::open(registry.data_dir().join("registry.sqlite3")).unwrap();
    let mut corruptions = Vec::new();
    for consumers in [
        Value::Null,
        json!([]),
        json!([{"id": "mac", "name": "My Mac", "target": "Other", "approval": "manual"}]),
        json!([{"id": "mac", "name": "Changed", "target": "Applications", "approval": "manual"}]),
        json!([{"id": "other", "name": "My Mac", "target": "Applications", "approval": "manual"}]),
        json!([{"id": "mac", "name": "My Mac", "target": "Applications", "approval": "automatic"}]),
    ] {
        let mut corrupt = original.clone();
        corrupt["stages"][0]["consumers"] = consumers;
        corruptions.push(corrupt);
    }
    let mut corrupt = original.clone();
    corrupt["stages"][0]
        .as_object_mut()
        .unwrap()
        .remove("consumers");
    corruptions.push(corrupt);
    for field in ["id", "name", "target", "approval"] {
        let mut corrupt = original.clone();
        corrupt["stages"][0]["consumers"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        corruptions.push(corrupt);
    }
    let request = RecordConsumerDeployment {
        app_id: app.manifest.app_id.clone(),
        stage_id: "build".to_owned(),
        consumer_id: "mac".to_owned(),
        expected_manifest_hash: app.manifest.manifest_hash.clone(),
        expected_updated_at: app.updated_at.clone(),
        release_ref: "1.2.3".to_owned(),
        notes: None,
        approved: true,
    };
    for corrupt in corruptions {
        let saved = corrupt.to_string();
        connection
            .execute("UPDATE registered_apps SET snapshot = ?1", [&saved])
            .unwrap();
        assert_eq!(registry.get("demo").unwrap_err().code(), "storage");
        assert_eq!(registry.list().unwrap_err().code(), "storage");
        assert_eq!(
            registry
                .record_consumer_deployment(&request)
                .unwrap_err()
                .code(),
            "storage"
        );
        assert!(registry.consumer_deployments(None).unwrap().is_empty());
        let unchanged: String = connection
            .query_row("SELECT snapshot FROM registered_apps", [], |row| row.get(0))
            .unwrap();
        assert_eq!(unchanged, saved);
    }
}
