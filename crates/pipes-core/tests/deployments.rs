mod support;

use std::{
    collections::BTreeSet,
    fs,
    sync::{Arc, Barrier},
    thread,
};

use chrono::{DateTime, Utc};
use pipes_core::{
    ConsumerApproval, ConsumerDeployment, RecordConsumerDeployment, RegisteredApp, Registry,
};
use rusqlite::{Connection, types::Value as SqlValue};
use serde_json::{Value, json};
use support::{FULL, Fixture, MINIMAL, canonical};

fn deployment_yaml() -> String {
    format!(
        "{}name: Demo Application\n",
        MINIMAL.replace(
            "[{id: build}]",
            r#"[
                {id: build},
                {id: beta, consumers: [{id: personal-mac, name: Personal MacBook, target: "MacBook / Applications"}]},
                {id: stable, consumers: [{id: personal-mac, name: Office Mac, target: "Office / Applications"}]}
            ]"#,
        )
    )
}

fn request(app: &RegisteredApp) -> RecordConsumerDeployment {
    RecordConsumerDeployment {
        app_id: app.manifest.app_id.clone(),
        stage_id: "beta".to_owned(),
        consumer_id: "personal-mac".to_owned(),
        expected_manifest_hash: app.manifest.manifest_hash.clone(),
        expected_updated_at: app.updated_at.clone(),
        release_ref: "1.2.3+build.45".to_owned(),
        notes: None,
        approved: true,
    }
}

fn database(registry: &Registry) -> Connection {
    Connection::open(registry.data_dir().join("registry.sqlite3")).unwrap()
}

fn count(connection: &Connection) -> i64 {
    connection
        .query_row("SELECT count(*) FROM consumer_deployments", [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn explicit_approval_records_exact_destination_and_provenance_without_changing_registration() {
    let fixture = Fixture::new(&deployment_yaml());
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    assert!(registry.consumer_deployments(None).unwrap().is_empty());
    let mut request = request(&app);
    request.release_ref = "  commit:0123456789abcdef  ".to_owned();
    request.notes = Some("  Installed manually\n\tVerified launch  ".to_owned());
    let before = Utc::now();
    let marker = registry.record_consumer_deployment(&request).unwrap();
    let after = Utc::now();
    let recorded = DateTime::parse_from_rfc3339(&marker.recorded_at).unwrap();
    assert_eq!(recorded.offset().local_minus_utc(), 0);
    assert!(recorded >= before && recorded <= after);
    assert_eq!(marker.consumer.approval, ConsumerApproval::Manual);
    assert_eq!(
        serde_json::to_value(&marker).unwrap(),
        json!({
            "id": "1",
            "appId": "demo",
            "appName": "Demo Application",
            "stageId": "beta",
            "consumer": {
                "id": "personal-mac", "name": "Personal MacBook",
                "target": "MacBook / Applications", "approval": "manual"
            },
            "releaseRef": "commit:0123456789abcdef",
            "notes": "Installed manually\n\tVerified launch",
            "manifestHash": app.manifest.manifest_hash,
            "manifestPath": canonical(&fixture.manifest),
            "sourcePath": canonical(&fixture.source),
            "registrationUpdatedAt": app.updated_at,
            "recordedAt": marker.recorded_at,
        })
    );
    assert_eq!(registry.get("demo").unwrap(), app);
    assert_eq!(
        registry.consumer_deployments(Some("demo")).unwrap(),
        std::slice::from_ref(&marker)
    );
    assert_eq!(
        serde_json::from_value::<ConsumerDeployment>(serde_json::to_value(&marker).unwrap())
            .unwrap(),
        marker
    );
    let mut stable = request;
    stable.stage_id = "stable".to_owned();
    let stable = registry.record_consumer_deployment(&stable).unwrap();
    assert_eq!(stable.consumer.target, "Office / Applications");
    assert_eq!(stable.stage_id, "stable");
    assert_eq!(registry.get("demo").unwrap(), app);
    drop(registry);
    let reopened = Registry::open(&fixture.state).unwrap();
    assert_eq!(
        reopened.consumer_deployments(None).unwrap(),
        [stable, marker]
    );
}

#[test]
fn approval_references_notes_and_review_inputs_are_required_and_validated_before_writes() {
    let fixture = Fixture::new(&deployment_yaml());
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    let original = request(&app);
    let mut unapproved = original.clone();
    unapproved.approved = false;
    assert_eq!(
        registry
            .record_consumer_deployment(&unapproved)
            .unwrap_err()
            .code(),
        "approval_required"
    );

    let mut invalid = Vec::new();
    for value in [
        "",
        " ",
        "\n",
        "release\n",
        "\trelease",
        "v1\r",
        "v1\0",
        "a\u{7f}b",
        "a\u{85}b",
    ] {
        let mut request = original.clone();
        request.release_ref = value.to_owned();
        invalid.push(request);
    }
    let mut too_long = original.clone();
    too_long.release_ref = "\u{e9}".repeat(257);
    invalid.push(too_long);
    for notes in [
        "a\0b".to_owned(),
        "a\rb".to_owned(),
        "a\u{7f}b".to_owned(),
        "a\u{85}b".to_owned(),
        "\u{e9}".repeat(4097),
    ] {
        let mut request = original.clone();
        request.notes = Some(notes);
        invalid.push(request);
    }
    for field in ["appId", "stageId", "consumerId"] {
        for value in ["", "Bad", "a--b", "a/b", "a ", "' OR 1=1 --", "\u{e9}"] {
            let mut request = serde_json::to_value(&original).unwrap();
            request[field] = json!(value);
            invalid.push(serde_json::from_value(request).unwrap());
        }
    }
    for value in [
        "".to_owned(),
        " ".repeat(64),
        "a".repeat(63),
        "A".repeat(64),
        "g".repeat(64),
    ] {
        let mut request = original.clone();
        request.expected_manifest_hash = value;
        invalid.push(request);
    }
    for value in [
        "",
        "invalid",
        "2026-01-01T12:00:00+01:00",
        " 2026-01-01T12:00:00Z",
    ] {
        let mut request = original.clone();
        request.expected_updated_at = value.to_owned();
        invalid.push(request);
    }
    for request in invalid {
        let error = registry.record_consumer_deployment(&request).unwrap_err();
        assert_eq!(error.code(), "invalid_deployment", "{request:?}: {error}");
    }
    assert!(registry.consumer_deployments(None).unwrap().is_empty());
    assert_eq!(registry.get("demo").unwrap(), app);
}

#[test]
fn reference_and_note_limits_count_unicode_characters_and_blank_notes_become_none() {
    let fixture = Fixture::new(&deployment_yaml());
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    let mut request = request(&app);
    request.release_ref = format!("  {}  ", "\u{1f4bb}".repeat(256));
    request.notes = Some(format!("  {}\n\t{}", "\u{e9}".repeat(4093), "\u{1f4bb}"));
    let marker = registry.record_consumer_deployment(&request).unwrap();
    assert_eq!(marker.release_ref.chars().count(), 256);
    assert_eq!(marker.notes.as_deref().unwrap().chars().count(), 4096);
    for notes in [None, Some("".to_owned()), Some(" \n\t  ".to_owned())] {
        request.notes = notes;
        assert!(
            registry
                .record_consumer_deployment(&request)
                .unwrap()
                .notes
                .is_none()
        );
    }
    assert_eq!(registry.consumer_deployments(None).unwrap().len(), 4);
}

#[test]
fn json_requests_require_approval_and_review_tokens_and_cannot_supply_a_destination_or_time() {
    let fixture = Fixture::new(&deployment_yaml());
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    let original = serde_json::to_value(request(&app)).unwrap();
    for field in [
        "appId",
        "stageId",
        "consumerId",
        "expectedManifestHash",
        "expectedUpdatedAt",
        "releaseRef",
        "approved",
    ] {
        let mut value = original.clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<RecordConsumerDeployment>(value).is_err(),
            "{field}"
        );
        let mut value = original.clone();
        value[field] = Value::Null;
        assert!(
            serde_json::from_value::<RecordConsumerDeployment>(value).is_err(),
            "{field}"
        );
    }
    for field in [
        "consumer",
        "target",
        "appName",
        "recordedAt",
        "id",
        "release_ref",
    ] {
        let mut value = original.clone();
        value[field] = json!("caller supplied");
        assert!(
            serde_json::from_value::<RecordConsumerDeployment>(value).is_err(),
            "{field}"
        );
    }
    let mut value = original;
    value["approved"] = json!("true");
    assert!(serde_json::from_value::<RecordConsumerDeployment>(value).is_err());
}

#[test]
fn stale_hash_revision_missing_consumer_and_unregistered_app_fail_without_partial_markers() {
    let fixture = Fixture::new(&deployment_yaml());
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    let original = request(&app);
    let mut changed_hash = original.clone();
    changed_hash.expected_manifest_hash = "0".repeat(64);
    let mut changed_revision = original.clone();
    changed_revision.expected_updated_at = "2020-01-01T00:00:00Z".to_owned();
    for stale in [changed_hash, changed_revision] {
        assert_eq!(
            registry
                .record_consumer_deployment(&stale)
                .unwrap_err()
                .code(),
            "registration_changed"
        );
    }
    for (stage, consumer) in [
        ("missing-stage", "personal-mac"),
        ("beta", "missing-consumer"),
        ("build", "personal-mac"),
    ] {
        let mut missing = original.clone();
        missing.stage_id = stage.to_owned();
        missing.consumer_id = consumer.to_owned();
        let error = registry.record_consumer_deployment(&missing).unwrap_err();
        assert_eq!(error.code(), "consumer_not_found");
        assert!(error.to_string().contains(stage) && error.to_string().contains(consumer));
    }
    fixture.write(MINIMAL);
    let replaced = registry.register(&fixture.manifest, None, true).unwrap();
    assert_eq!(
        registry
            .record_consumer_deployment(&original)
            .unwrap_err()
            .code(),
        "registration_changed"
    );
    assert_eq!(
        registry
            .record_consumer_deployment(&request(&replaced))
            .unwrap_err()
            .code(),
        "consumer_not_found"
    );
    registry.unregister("demo").unwrap();
    assert_eq!(
        registry
            .record_consumer_deployment(&original)
            .unwrap_err()
            .code(),
        "not_found"
    );
    assert!(registry.consumer_deployments(None).unwrap().is_empty());
    assert_eq!(count(&database(&registry)), 0);
}

#[test]
fn identical_yaml_relocated_to_another_manifest_and_source_invalidates_the_review() {
    let fixture = Fixture::new(&deployment_yaml());
    let registry = Registry::open(&fixture.state).unwrap();
    let first = registry.register(&fixture.manifest, None, false).unwrap();
    let location = fixture.path("relocated");
    fs::create_dir(&location).unwrap();
    fs::create_dir(location.join("source")).unwrap();
    let path = location.join("app.yaml");
    fs::write(&path, deployment_yaml()).unwrap();
    let replaced = registry.register(&path, None, true).unwrap();
    assert_eq!(
        first.manifest.manifest_hash,
        replaced.manifest.manifest_hash
    );
    assert_ne!(
        first.manifest.manifest_path,
        replaced.manifest.manifest_path
    );
    assert_ne!(first.manifest.source_path, replaced.manifest.source_path);
    assert_ne!(first.updated_at, replaced.updated_at);
    assert_eq!(
        registry
            .record_consumer_deployment(&request(&first))
            .unwrap_err()
            .code(),
        "registration_changed"
    );
    assert!(registry.consumer_deployments(None).unwrap().is_empty());
    let marker = registry
        .record_consumer_deployment(&request(&replaced))
        .unwrap();
    assert_eq!(marker.manifest_path, replaced.manifest.manifest_path);
    assert_eq!(marker.source_path, replaced.manifest.source_path);
}

#[cfg(unix)]
#[test]
fn changing_only_the_resolved_source_path_requires_replacement_and_invalidates_review() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new(&deployment_yaml().replace("path: source", "path: live-source"));
    let link = fixture.path("live-source");
    symlink(&fixture.source, &link).unwrap();
    let registry = Registry::open(&fixture.state).unwrap();
    let first = registry.register(&fixture.manifest, None, false).unwrap();
    let source = fixture.path("new-source");
    fs::create_dir(&source).unwrap();
    fs::remove_file(&link).unwrap();
    symlink(&source, &link).unwrap();
    assert_eq!(
        registry
            .register(&fixture.manifest, None, false)
            .unwrap_err()
            .code(),
        "already_registered"
    );
    let replaced = registry.register(&fixture.manifest, None, true).unwrap();
    assert_eq!(
        first.manifest.manifest_hash,
        replaced.manifest.manifest_hash
    );
    assert_eq!(
        first.manifest.manifest_path,
        replaced.manifest.manifest_path
    );
    assert_ne!(first.manifest.source_path, replaced.manifest.source_path);
    assert_ne!(first.updated_at, replaced.updated_at);
    assert_eq!(
        registry
            .record_consumer_deployment(&request(&first))
            .unwrap_err()
            .code(),
        "registration_changed"
    );
    assert_eq!(
        registry
            .record_consumer_deployment(&request(&replaced))
            .unwrap()
            .source_path,
        canonical(&source)
    );
}

#[test]
fn recording_uses_saved_yaml_even_after_source_and_manifest_deletion_and_executes_nothing() {
    let fixture = Fixture::new(FULL);
    let sentinel = fixture.path("MUST-NOT-EXIST");
    let script = format!("touch '{}'", sentinel.display());
    let exec = serde_json::to_string(&["sh", "-c", script.as_str()]).unwrap();
    let target = serde_json::to_string(&script).unwrap();
    let yaml = FULL
        .replace(
            "  - id: shipped\n",
            &format!("  - id: shipped\n    consumers: [{{id: personal-mac, target: {target}}}]\n"),
        )
        .replace("[cargo, test]", &exec)
        .replace(
            "[tool-not-installed, \"argument with spaces\", \"\"]",
            &exec,
        );
    fixture.write(&yaml);
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    let mut request = request(&app);
    request.stage_id = "shipped".to_owned();
    request.release_ref = format!("$({script})");
    request.notes = Some(script.clone());
    assert!(!fixture.source.join("VERSION").exists());
    assert!(!fixture.source.join(".git").exists());
    fs::remove_file(&fixture.manifest).unwrap();
    fs::remove_dir(&fixture.source).unwrap();
    let marker = registry.record_consumer_deployment(&request).unwrap();
    assert_eq!(marker.consumer.target, script);
    assert_eq!(registry.get("sample-app").unwrap(), app);
    registry.unregister("sample-app").unwrap();
    let reopened = Registry::open(&fixture.state).unwrap();
    assert_eq!(
        reopened.consumer_deployments(Some("sample-app")).unwrap(),
        [marker]
    );
    assert!(!sentinel.exists());
    let connection = database(&reopened);
    let tables: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        tables,
        [
            "consumer_deployments",
            "harness_results",
            "pipeline_run_logs",
            "pipeline_runs",
            "promotion_approvals",
            "registered_apps",
            "sqlite_sequence"
        ]
    );
    assert!(reopened.pipeline_runs(None).unwrap().is_empty());
}

#[test]
fn history_remains_immutable_across_replacement_consumer_removal_unregistration_and_reregistration()
{
    let original_yaml = deployment_yaml();
    let fixture = Fixture::new(&original_yaml);
    let registry = Registry::open(&fixture.state).unwrap();
    let first = registry.register(&fixture.manifest, None, false).unwrap();
    let first_marker = registry
        .record_consumer_deployment(&request(&first))
        .unwrap();
    fixture.write(
        &original_yaml
            .replace("Demo Application", "Renamed App")
            .replace("Personal MacBook", "Replacement Laptop")
            .replace("MacBook / Applications", "Other Laptop / Apps"),
    );
    let replacement = registry.register(&fixture.manifest, None, true).unwrap();
    let second_marker = registry
        .record_consumer_deployment(&request(&replacement))
        .unwrap();
    assert_eq!(second_marker.app_name, "Renamed App");
    assert_eq!(second_marker.consumer.name, "Replacement Laptop");
    assert_eq!(second_marker.consumer.target, "Other Laptop / Apps");
    assert_eq!(first_marker.consumer.name, "Personal MacBook");
    assert_eq!(first_marker.consumer.target, "MacBook / Applications");
    fixture.write(MINIMAL);
    registry.register(&fixture.manifest, None, true).unwrap();
    assert_eq!(
        registry.consumer_deployments(None).unwrap(),
        [second_marker.clone(), first_marker.clone()]
    );
    registry.unregister("demo").unwrap();
    assert!(registry.list().unwrap().is_empty());
    assert_eq!(
        registry.consumer_deployments(Some("demo")).unwrap(),
        [second_marker.clone(), first_marker.clone()]
    );
    fixture.write(&original_yaml);
    let reregistered = registry.register(&fixture.manifest, None, false).unwrap();
    assert_eq!(
        reregistered.manifest.manifest_hash,
        first.manifest.manifest_hash
    );
    assert_ne!(reregistered.updated_at, first.updated_at);
    assert_eq!(
        registry
            .record_consumer_deployment(&request(&first))
            .unwrap_err()
            .code(),
        "registration_changed"
    );
    let third_marker = registry
        .record_consumer_deployment(&request(&reregistered))
        .unwrap();
    assert_eq!(third_marker.id, "3");
    let retained_history = Registry::open(&fixture.state)
        .unwrap()
        .consumer_deployments(None)
        .unwrap();
    assert_eq!(
        retained_history,
        [third_marker, second_marker, first_marker]
    );
}

#[test]
fn repeated_refs_append_and_numeric_ordering_and_archived_app_filters_are_durable() {
    let fixture = Fixture::new(&deployment_yaml());
    let registry = Registry::open(&fixture.state).unwrap();
    let first = registry.register(&fixture.manifest, None, false).unwrap();
    let other_path = fixture.path("other.yaml");
    fs::write(
        &other_path,
        deployment_yaml().replace("app: demo", "app: other"),
    )
    .unwrap();
    let other = registry.register(&other_path, None, false).unwrap();
    let mut expected = Vec::new();
    for index in 0..12 {
        let app = if index % 2 == 0 { &first } else { &other };
        let marker = registry.record_consumer_deployment(&request(app)).unwrap();
        assert_eq!(marker.id, (index + 1).to_string());
        expected.push(marker);
    }
    expected.reverse();
    registry.unregister("demo").unwrap();
    let reopened = Registry::open(&fixture.state).unwrap();
    assert_eq!(reopened.consumer_deployments(None).unwrap(), expected);
    for app_id in ["demo", "other"] {
        let selected: Vec<_> = expected
            .iter()
            .filter(|marker| marker.app_id == app_id)
            .cloned()
            .collect();
        assert_eq!(
            reopened.consumer_deployments(Some(app_id)).unwrap(),
            selected
        );
    }
    assert!(
        reopened
            .consumer_deployments(Some("never-registered"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        reopened
            .consumer_deployments(Some("' OR 1=1 --"))
            .unwrap_err()
            .code(),
        "invalid_deployment"
    );
}

#[test]
fn insertion_order_not_timestamps_controls_listing_and_ids_remain_opaque_above_js_precision() {
    let fixture = Fixture::new(&deployment_yaml());
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    let request = request(&app);
    let first = registry.record_consumer_deployment(&request).unwrap();
    let connection = database(&registry);
    connection
        .execute(
            "UPDATE sqlite_sequence SET seq = 9007199254740991 WHERE name = 'consumer_deployments'",
            [],
        )
        .unwrap();
    let second = registry.record_consumer_deployment(&request).unwrap();
    let third = registry.record_consumer_deployment(&request).unwrap();
    assert_eq!(second.id, "9007199254740992");
    assert_eq!(third.id, "9007199254740993");
    assert_eq!(
        serde_json::to_value(&third).unwrap()["id"],
        json!("9007199254740993")
    );
    connection
        .execute(
            "UPDATE consumer_deployments SET recorded_at = '2000-01-01T00:00:00Z' WHERE id = ?1",
            [&third.id],
        )
        .unwrap();
    let ids: Vec<_> = registry
        .consumer_deployments(None)
        .unwrap()
        .into_iter()
        .map(|marker| marker.id)
        .collect();
    assert_eq!(ids, [third.id, second.id, first.id]);
}

#[test]
fn concurrent_independent_handles_append_every_approval_once_without_deduplicating_refs() {
    let fixture = Fixture::new(&deployment_yaml());
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    let count = 8;
    let barrier = Arc::new(Barrier::new(count));
    let threads: Vec<_> = (0..count)
        .map(|_| {
            let state = fixture.state.clone();
            let request = request(&app);
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                Registry::open(state)?.record_consumer_deployment(&request)
            })
        })
        .collect();
    let markers: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap().unwrap())
        .collect();
    let ids: BTreeSet<_> = markers.iter().map(|marker| marker.id.clone()).collect();
    assert_eq!(ids.len(), count);
    let saved = registry.consumer_deployments(None).unwrap();
    assert_eq!(saved.len(), count);
    assert!(markers.iter().all(|marker| saved.contains(marker)));
    assert_eq!(registry.get("demo").unwrap(), app);
}

#[test]
fn autoincrement_exhaustion_is_an_explicit_storage_failure_with_no_partial_row() {
    let fixture = Fixture::new(&deployment_yaml());
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    let request = request(&app);
    registry.record_consumer_deployment(&request).unwrap();
    let connection = database(&registry);
    connection
        .execute(
            "UPDATE sqlite_sequence SET seq = ?1 WHERE name = 'consumer_deployments'",
            [i64::MAX],
        )
        .unwrap();
    assert_eq!(
        registry
            .record_consumer_deployment(&request)
            .unwrap_err()
            .code(),
        "storage"
    );
    assert_eq!(count(&connection), 1);
    assert_eq!(registry.get("demo").unwrap(), app);
}

#[test]
fn corrupted_history_values_fail_explicitly_instead_of_skipping_or_normalizing_rows() {
    let fixture = Fixture::new(&deployment_yaml());
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    registry.record_consumer_deployment(&request(&app)).unwrap();
    let connection = database(&registry);
    let mut corruptions = vec![
        ("id", SqlValue::Integer(0)),
        ("id", SqlValue::Integer(-1)),
        ("consumer_name", SqlValue::Blob(b"not text".to_vec())),
        ("recorded_at", SqlValue::Blob(b"not text".to_vec())),
    ];
    for (column, value) in [
        ("app_id", "not a slug"),
        ("app_name", ""),
        ("stage_id", "Bad"),
        ("consumer_id", ""),
        ("consumer_name", " "),
        ("consumer_name", "a\nb"),
        ("consumer_target", ""),
        ("consumer_target", "a\tb"),
        ("consumer_approval", "automatic"),
        ("release_ref", ""),
        ("release_ref", " padded "),
        ("release_ref", "a\nb"),
        ("notes", " "),
        ("notes", " padded "),
        ("notes", "a\rb"),
        ("manifest_hash", "bad"),
        ("manifest_path", "relative"),
        ("source_path", "/a/../b"),
        ("registration_updated_at", "invalid"),
        ("recorded_at", "invalid"),
        ("recorded_at", "2026-01-01T00:00:00+01:00"),
    ] {
        corruptions.push((column, SqlValue::Text(value.to_owned())));
    }
    for (column, limit) in [
        ("consumer_name", 128),
        ("consumer_target", 1024),
        ("release_ref", 256),
        ("notes", 4096),
    ] {
        corruptions.push((column, SqlValue::Text("\u{e9}".repeat(limit + 1))));
    }
    for (column, value) in corruptions {
        let original: SqlValue = connection
            .query_row(
                &format!("SELECT {column} FROM consumer_deployments"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        connection
            .execute(
                &format!("UPDATE consumer_deployments SET {column} = ?1"),
                [&value],
            )
            .unwrap();
        let error = registry.consumer_deployments(None).unwrap_err();
        assert_eq!(error.code(), "storage", "{column}: {error}");
        if column != "app_id" {
            assert_eq!(
                registry
                    .consumer_deployments(Some("demo"))
                    .unwrap_err()
                    .code(),
                "storage"
            );
        }
        assert_eq!(count(&connection), 1);
        let unchanged: SqlValue = connection
            .query_row(
                &format!("SELECT {column} FROM consumer_deployments"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unchanged, value);
        connection
            .execute(
                &format!("UPDATE consumer_deployments SET {column} = ?1"),
                [original],
            )
            .unwrap();
    }
    assert_eq!(registry.consumer_deployments(None).unwrap().len(), 1);
}

#[test]
fn incompatible_history_table_declarations_are_never_silently_accepted_or_reset() {
    for (from, to) in [
        (" AUTOINCREMENT", ""),
        (
            "id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT",
            "id TEXT NOT NULL PRIMARY KEY",
        ),
        ("app_id TEXT NOT NULL", "app_id TEXT"),
        ("notes TEXT", "notes TEXT NOT NULL"),
        ("notes TEXT", "notesTEXT"),
        (
            "consumer_target TEXT NOT NULL",
            "consumer_target BLOB NOT NULL",
        ),
        (
            "recorded_at TEXT NOT NULL",
            "recorded_at TEXT NOT NULL, extra TEXT",
        ),
        ("recorded_at TEXT NOT NULL", "recorded_atTEXTNOTNULL"),
        (
            "app_id TEXT NOT NULL",
            "app_id TEXT NOT NULL REFERENCES registered_apps(app_id) ON DELETE CASCADE",
        ),
        (
            "consumer_approval TEXT NOT NULL",
            "consumer_approval TEXT NOT NULL DEFAULT 'manual'",
        ),
    ] {
        let fixture = Fixture::new(&deployment_yaml());
        let registry = Registry::open(&fixture.state).unwrap();
        let app = registry.register(&fixture.manifest, None, false).unwrap();
        registry.record_consumer_deployment(&request(&app)).unwrap();
        let connection = database(&registry);
        let original: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'consumer_deployments'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let changed = original.replace(from, to);
        assert_ne!(original, changed, "{from}");
        connection
            .execute_batch("ALTER TABLE consumer_deployments RENAME TO saved_history")
            .unwrap();
        connection.execute_batch(&changed).unwrap();
        connection
            .execute_batch(
                "DROP INDEX consumer_deployments_app_id;
             CREATE INDEX consumer_deployments_app_id ON consumer_deployments(app_id);",
            )
            .unwrap();
        assert_eq!(
            Registry::open(&fixture.state).unwrap_err().code(),
            "storage",
            "{from}"
        );
        assert_eq!(
            registry.consumer_deployments(None).unwrap_err().code(),
            "storage",
            "{from}"
        );
        assert_eq!(
            registry
                .record_consumer_deployment(&request(&app))
                .unwrap_err()
                .code(),
            "storage",
            "{from}"
        );
        let saved: i64 = connection
            .query_row("SELECT count(*) FROM saved_history", [], |row| row.get(0))
            .unwrap();
        assert_eq!(saved, 1);
        assert_eq!(count(&connection), 0);
    }
}

#[test]
fn registration_declarations_reject_hidden_or_incompatible_column_semantics() {
    for (from, to) in [
        ("snapshot TEXT NOT NULL", "snapshotTEXTNOTNULL"),
        ("snapshot TEXT NOT NULL", "snapshot TEXT"),
        ("snapshot TEXT NOT NULL", "snapshot BLOB NOT NULL"),
        (
            "snapshot TEXT NOT NULL",
            "snapshot TEXT NOT NULL DEFAULT '{}'",
        ),
        (
            "updated_at TEXT NOT NULL",
            "updated_at TEXT NOT NULL, extra TEXT",
        ),
        (
            "manifest_hash TEXT NOT NULL",
            "manifest_hash TEXT NOT NULL REFERENCES consumer_deployments(manifest_hash)",
        ),
    ] {
        let fixture = Fixture::new(&deployment_yaml());
        let registry = Registry::open(&fixture.state).unwrap();
        let app = registry.register(&fixture.manifest, None, false).unwrap();
        registry.record_consumer_deployment(&request(&app)).unwrap();
        let connection = database(&registry);
        let original: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'registered_apps'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let changed = original.replace(from, to);
        assert_ne!(original, changed);
        connection
            .execute_batch("ALTER TABLE registered_apps RENAME TO saved_registration")
            .unwrap();
        connection.execute_batch(&changed).unwrap();
        assert_eq!(
            Registry::open(&fixture.state).unwrap_err().code(),
            "storage",
            "{from}"
        );
        assert_eq!(
            registry.consumer_deployments(None).unwrap_err().code(),
            "storage",
            "{from}"
        );
        assert_eq!(
            registry
                .record_consumer_deployment(&request(&app))
                .unwrap_err()
                .code(),
            "storage",
            "{from}"
        );
        let saved: i64 = connection
            .query_row("SELECT count(*) FROM saved_registration", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(saved, 1);
        assert_eq!(count(&connection), 1);
    }
}

#[test]
fn missing_tables_indexes_and_added_constraints_or_triggers_block_all_registry_mutations() {
    for sql in [
        "DROP TABLE consumer_deployments",
        "DROP INDEX consumer_deployments_app_id",
        "DROP INDEX consumer_deployments_app_id; CREATE INDEX consumer_deployments_app_id ON consumer_deployments(release_ref)",
        "CREATE UNIQUE INDEX forbidden_dedup ON consumer_deployments(app_id, release_ref)",
        "CREATE TRIGGER fake_success BEFORE INSERT ON consumer_deployments BEGIN SELECT RAISE(IGNORE); END",
        "CREATE TRIGGER erase_history AFTER DELETE ON registered_apps BEGIN DELETE FROM consumer_deployments; END",
        "CREATE UNIQUE INDEX forbidden_names ON registered_apps(updated_at)",
    ] {
        let fixture = Fixture::new(&deployment_yaml());
        let registry = Registry::open(&fixture.state).unwrap();
        let app = registry.register(&fixture.manifest, None, false).unwrap();
        registry.record_consumer_deployment(&request(&app)).unwrap();
        let connection = database(&registry);
        connection.execute_batch(sql).unwrap();
        assert_eq!(
            Registry::open(&fixture.state).unwrap_err().code(),
            "storage",
            "{sql}"
        );
        assert_eq!(
            registry.consumer_deployments(None).unwrap_err().code(),
            "storage",
            "{sql}"
        );
        assert_eq!(
            registry
                .record_consumer_deployment(&request(&app))
                .unwrap_err()
                .code(),
            "storage",
            "{sql}"
        );
        assert_eq!(
            registry.unregister("demo").unwrap_err().code(),
            "storage",
            "{sql}"
        );
        assert_eq!(
            registry
                .register(&fixture.manifest, None, true)
                .unwrap_err()
                .code(),
            "storage",
            "{sql}"
        );
        let registered: i64 = connection
            .query_row("SELECT count(*) FROM registered_apps", [], |row| row.get(0))
            .unwrap();
        assert_eq!(registered, 1);
        if !sql.starts_with("DROP TABLE") {
            assert_eq!(count(&connection), 1);
        }
    }
}

#[test]
fn schema_changes_on_existing_handles_are_explicit_and_never_automatically_migrated() {
    let fixture = Fixture::new(&deployment_yaml());
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    let connection = database(&registry);
    for version in [5, 7] {
        connection
            .pragma_update(None, "user_version", version)
            .unwrap();
        assert_eq!(
            registry.consumer_deployments(None).unwrap_err().code(),
            "unsupported_schema"
        );
        assert_eq!(
            registry
                .record_consumer_deployment(&request(&app))
                .unwrap_err()
                .code(),
            "unsupported_schema"
        );
        assert_eq!(count(&connection), 0);
    }
}
