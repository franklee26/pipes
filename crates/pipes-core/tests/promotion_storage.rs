mod promotion;
#[allow(dead_code)]
mod support;

use std::{
    collections::BTreeSet,
    fs,
    sync::{Arc, Barrier},
    thread,
};

use pipes_core::Registry;
use promotion::*;
use rusqlite::{Connection, types::Value as SqlValue};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn count(connection: &Connection, table: &str) -> i64 {
    connection
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn evidence_and_approvals_survive_check_policy_and_consumer_removal_and_unregister() {
    let (fixture, registry, app) = setup(&format!("{HARNESS}, {REQUIRED}, {OPTIONAL}"));
    let context = context(&app);
    let result = registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    let marker = registry
        .record_consumer_deployment(&deployment(&context, "lab-device"))
        .unwrap();
    let assessment = registry.assess_promotion(&context).unwrap();
    let mut request = approval(&assessment);
    request.acknowledge_optional = true;
    let saved = registry.approve_promotion(&request).unwrap();
    fixture.write(support::MINIMAL);
    registry.register(&fixture.manifest, None, true).unwrap();
    assert_eq!(
        registry.harness_results(None).unwrap(),
        std::slice::from_ref(&result)
    );
    assert_eq!(
        registry.promotion_approvals(None).unwrap(),
        std::slice::from_ref(&saved)
    );
    registry.unregister("demo").unwrap();
    fs::remove_file(&fixture.manifest).unwrap();
    fs::remove_dir(&fixture.source).unwrap();
    let reopened = Registry::open(&fixture.state).unwrap();
    assert!(reopened.list().unwrap().is_empty());
    assert_eq!(reopened.harness_results(Some("demo")).unwrap(), [result]);
    assert_eq!(
        reopened.consumer_deployments(Some("demo")).unwrap(),
        [marker]
    );
    assert_eq!(reopened.promotion_approvals(Some("demo")).unwrap(), [saved]);
    assert!(
        reopened
            .harness_results(Some("not-registered"))
            .unwrap()
            .is_empty()
    );
    assert!(
        reopened
            .promotion_approvals(Some("not-registered"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        reopened
            .harness_results(Some("not a slug"))
            .unwrap_err()
            .code(),
        "invalid_harness_result"
    );
    assert_eq!(
        reopened
            .promotion_approvals(Some("not a slug"))
            .unwrap_err()
            .code(),
        "invalid_promotion"
    );
}

#[test]
fn append_only_histories_sort_numerically_and_filter_archived_apps() {
    let (fixture, registry, app) = setup(HARNESS);
    let other_path = fixture.path("other.yaml");
    fs::write(
        &other_path,
        yaml(HARNESS).replace("app: demo", "app: other"),
    )
    .unwrap();
    let other = registry.register(&other_path, None, false).unwrap();
    let mut results = Vec::new();
    let mut approvals = Vec::new();
    for index in 0..12 {
        let app = if index % 2 == 0 { &app } else { &other };
        let context = context(app);
        let result = registry
            .record_harness_result(&report(&context, 50, 45))
            .unwrap();
        assert_eq!(result.id, (index + 1).to_string());
        results.push(result);
        let assessment = registry.assess_promotion(&context).unwrap();
        approvals.push(registry.approve_promotion(&approval(&assessment)).unwrap());
    }
    results.reverse();
    approvals.reverse();
    registry.unregister("demo").unwrap();
    registry.unregister("other").unwrap();
    let reopened = Registry::open(&fixture.state).unwrap();
    assert_eq!(reopened.harness_results(None).unwrap(), results);
    assert_eq!(reopened.promotion_approvals(None).unwrap(), approvals);
    for id in ["demo", "other"] {
        assert_eq!(
            reopened.harness_results(Some(id)).unwrap(),
            results
                .iter()
                .filter(|result| result.app_id == id)
                .cloned()
                .collect::<Vec<_>>()
        );
        assert_eq!(
            reopened.promotion_approvals(Some(id)).unwrap(),
            approvals
                .iter()
                .filter(|approval| approval.assessment.app_id == id)
                .cloned()
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn history_ids_stay_opaque_above_js_precision_and_exhaustion_does_not_partial_write() {
    let (_fixture, registry, app) = setup(HARNESS);
    let context = context(&app);
    registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    let assessment = registry.assess_promotion(&context).unwrap();
    registry.approve_promotion(&approval(&assessment)).unwrap();
    let connection = database(&registry);
    for table in ["harness_results", "promotion_approvals"] {
        connection
            .execute(
                "UPDATE sqlite_sequence SET seq = 9007199254740991 WHERE name = ?1",
                [table],
            )
            .unwrap();
    }
    let result = registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    assert_eq!(result.id, "9007199254740992");
    let assessment = registry.assess_promotion(&context).unwrap();
    let saved = registry.approve_promotion(&approval(&assessment)).unwrap();
    assert_eq!(saved.id, "9007199254740992");
    assert_eq!(
        serde_json::to_value(&saved).unwrap()["id"],
        "9007199254740992"
    );
    connection.execute("UPDATE sqlite_sequence SET seq = ?1 WHERE name IN ('harness_results', 'promotion_approvals')", [i64::MAX]).unwrap();
    assert_eq!(
        registry
            .record_harness_result(&report(&context, 50, 45))
            .unwrap_err()
            .code(),
        "storage"
    );
    assert_eq!(
        registry
            .approve_promotion(&approval(&assessment))
            .unwrap_err()
            .code(),
        "storage"
    );
    assert_eq!(count(&connection, "harness_results"), 2);
    assert_eq!(count(&connection, "promotion_approvals"), 2);
    assert_eq!(registry.get("demo").unwrap(), app);
}

#[test]
fn invalid_approval_hash_notes_and_reference_do_not_append() {
    let (_fixture, registry, app) = setup(HARNESS);
    let context = context(&app);
    registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    let assessment = registry.assess_promotion(&context).unwrap();
    let original = approval(&assessment);
    let mut invalid = Vec::new();
    for (field, value) in [
        ("expectedAssessmentHash", json!("bad")),
        ("releaseRef", json!("")),
        ("expectedUpdatedAt", json!("2026-01-01T01:00:00+01:00")),
        ("notes", json!("a\rb")),
        ("notes", json!("\u{e9}".repeat(4097))),
    ] {
        let mut request = serde_json::to_value(&original).unwrap();
        request[field] = value;
        invalid.push(serde_json::from_value(request).unwrap());
    }
    for request in invalid {
        assert_eq!(
            registry.approve_promotion(&request).unwrap_err().code(),
            "invalid_promotion"
        );
    }
    let mut request = original;
    request.expected_assessment_hash = "f".repeat(64);
    assert_eq!(
        registry.approve_promotion(&request).unwrap_err().code(),
        "assessment_changed"
    );
    assert!(registry.promotion_approvals(None).unwrap().is_empty());
}

#[test]
fn corrupt_result_rows_never_fall_back_to_an_older_pass_or_disappear() {
    let (fixture, registry, app) = setup(HARNESS);
    let context = context(&app);
    registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    let assessed = registry.assess_promotion(&context).unwrap();
    let connection = database(&registry);
    let mut corruptions = vec![
        ("id", SqlValue::Integer(0)),
        ("id", SqlValue::Integer(-1)),
        ("total", SqlValue::Integer(-1)),
        ("total", SqlValue::Integer(4294967296)),
        ("total", SqlValue::Real(50.5)),
        ("total", SqlValue::Blob(vec![50])),
        ("passed", SqlValue::Integer(-1)),
        ("passed", SqlValue::Integer(51)),
        ("passed", SqlValue::Real(45.5)),
        ("notes", SqlValue::Blob(vec![0])),
    ];
    for (field, value) in [
        ("app_id", "not a slug"),
        ("app_name", ""),
        ("stage_id", "Bad"),
        ("check_id", ""),
        ("release_ref", ""),
        ("release_ref", " padded "),
        ("notes", " "),
        ("notes", "a\rb"),
        ("manifest_hash", "bad"),
        ("manifest_path", "relative"),
        ("source_path", "/a/../b"),
        ("registration_updated_at", "invalid"),
        ("recorded_at", "2026-01-01T01:00:00+01:00"),
    ] {
        corruptions.push((field, SqlValue::Text(value.to_owned())));
    }
    for (field, value) in corruptions {
        let old: SqlValue = connection
            .query_row(
                &format!("SELECT {field} FROM harness_results WHERE id = 2"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        connection
            .execute(
                &format!("UPDATE harness_results SET {field} = ?1 WHERE id = 2"),
                [&value],
            )
            .unwrap();
        assert_eq!(
            registry.harness_results(None).unwrap_err().code(),
            "storage",
            "{field}"
        );
        assert_eq!(
            registry.assess_promotion(&context).unwrap_err().code(),
            "storage",
            "{field}"
        );
        assert_eq!(
            registry
                .approve_promotion(&approval(&assessed))
                .unwrap_err()
                .code(),
            "storage",
            "{field}"
        );
        assert_eq!(
            Registry::open(&fixture.state).unwrap_err().code(),
            "storage",
            "{field}"
        );
        assert_eq!(count(&connection, "harness_results"), 2);
        assert_eq!(count(&connection, "promotion_approvals"), 0);
        let where_clause = if field == "id" { "id != 1" } else { "id = 2" };
        connection
            .execute(
                &format!("UPDATE harness_results SET {field} = ?1 WHERE {where_clause}"),
                [old],
            )
            .unwrap();
    }
    assert_eq!(registry.assess_promotion(&context).unwrap(), assessed);
}

fn rehash(value: &mut Value) {
    value.as_object_mut().unwrap().remove("assessmentHash");
    value["assessmentHash"] = json!(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).unwrap())
    ));
}

#[test]
fn corrupt_approval_snapshots_and_impossible_rehashed_statuses_are_rejected_without_repair() {
    let (fixture, registry, app) = setup(&format!("{HARNESS}, {OPTIONAL}, {SPARE}"));
    let context = context(&app);
    registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    let assessment = registry.assess_promotion(&context).unwrap();
    let mut request = approval(&assessment);
    request.acknowledge_optional = true;
    let saved = registry.approve_promotion(&request).unwrap();
    let connection = database(&registry);
    let original = serde_json::to_value(&assessment).unwrap();
    let mut corruptions = vec!["{".to_owned()];
    for (field, value) in [
        ("status", json!("ready")),
        ("status", json!("blocked")),
        ("status", json!("not_configured")),
        ("status", json!({"ready_with_warnings": null})),
        ("status", json!("invented")),
        ("assessmentHash", json!("a".repeat(64))),
        ("criteria", json!([])),
        ("criteria", Value::Null),
        ("stageRequirements", json!(["all.passed"])),
        ("unknown", json!(true)),
        ("sourcePath", json!("relative")),
        ("registrationUpdatedAt", json!("invalid")),
    ] {
        let mut value_to_save = original.clone();
        value_to_save[field] = value;
        if field != "assessmentHash" {
            rehash(&mut value_to_save);
        }
        corruptions.push(value_to_save.to_string());
    }
    for (field, value) in [
        ("status", json!("warning")),
        ("status", json!("blocked")),
        ("evidenceId", Value::Null),
        ("evidenceId", json!("01")),
        ("evidenceId", json!("999")),
        ("observed", Value::Null),
        ("observed", json!({"total": 49, "passed": 45})),
        ("observed", json!({"total": 50, "passed": 44})),
        ("observed", json!({"total": 50, "passed": 51})),
        ("message", json!("Claim legacy gates passed")),
        (
            "consumer",
            json!({"id": "lab-device", "name": "Lab", "target": "target", "approval": "manual"}),
        ),
    ] {
        let mut value_to_save = original.clone();
        value_to_save["criteria"][0][field] = value;
        rehash(&mut value_to_save);
        corruptions.push(value_to_save.to_string());
    }
    for field in ["observed", "evidenceId", "consumer", "criterion", "status"] {
        let mut value_to_save = original.clone();
        value_to_save["criteria"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        rehash(&mut value_to_save);
        corruptions.push(value_to_save.to_string());
    }
    for value in corruptions {
        connection
            .execute("UPDATE promotion_approvals SET assessment = ?1", [&value])
            .unwrap();
        let error = registry.promotion_approvals(None).unwrap_err();
        assert_eq!(error.code(), "storage", "{value}: {error}");
        assert_eq!(
            Registry::open(&fixture.state).unwrap_err().code(),
            "storage"
        );
        let persisted: String = connection
            .query_row("SELECT assessment FROM promotion_approvals", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(persisted, value);
    }
    connection
        .execute(
            "UPDATE promotion_approvals SET assessment = ?1",
            [original.to_string()],
        )
        .unwrap();
    assert_eq!(registry.promotion_approvals(None).unwrap(), [saved]);
}

#[test]
fn frozen_optional_skip_lists_and_approval_metadata_are_strict() {
    let (fixture, registry, app) = setup(&format!("{OPTIONAL}, {SPARE}"));
    let assessment = registry.assess_promotion(&context(&app)).unwrap();
    let mut request = approval(&assessment);
    request.acknowledge_optional = true;
    let saved = registry.approve_promotion(&request).unwrap();
    let connection = database(&registry);
    let mut corruptions = Vec::new();
    for value in [
        "null",
        "[]",
        "{}",
        "[\"spare-delivery\",\"personal-delivery\"]",
        "[\"personal-delivery\"]",
        "[\"personal-delivery\",\"spare-delivery\",\"spare-delivery\"]",
        "[\"smoke-tests\"]",
    ] {
        corruptions.push(("skipped_optional", SqlValue::Text(value.to_owned())));
    }
    for (field, value) in [
        ("app_id", "foreign"),
        ("notes", " unnormalized "),
        ("notes", "a\rb"),
        ("recorded_at", "invalid"),
        ("recorded_at", "2026-01-01T01:00:00+01:00"),
    ] {
        corruptions.push((field, SqlValue::Text(value.to_owned())));
    }
    corruptions.push(("id", SqlValue::Integer(0)));
    corruptions.push(("assessment", SqlValue::Blob(vec![0])));
    for (field, value) in corruptions {
        let old: SqlValue = connection
            .query_row(
                &format!("SELECT {field} FROM promotion_approvals"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        connection
            .execute(
                &format!("UPDATE promotion_approvals SET {field} = ?1"),
                [&value],
            )
            .unwrap();
        assert_eq!(
            registry.promotion_approvals(None).unwrap_err().code(),
            "storage",
            "{field}"
        );
        assert_eq!(
            Registry::open(&fixture.state).unwrap_err().code(),
            "storage"
        );
        assert_eq!(count(&connection, "promotion_approvals"), 1);
        connection
            .execute(
                &format!("UPDATE promotion_approvals SET {field} = ?1"),
                [old],
            )
            .unwrap();
    }
    assert_eq!(registry.promotion_approvals(None).unwrap(), [saved]);
}

#[test]
fn frozen_approvals_detect_missing_or_mismatched_evidence_without_current_registrations() {
    let (_fixture, registry, app) = setup(&format!("{HARNESS}, {REQUIRED}"));
    let context = context(&app);
    registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    registry
        .record_consumer_deployment(&deployment(&context, "lab-device"))
        .unwrap();
    let assessment = registry.assess_promotion(&context).unwrap();
    registry.approve_promotion(&approval(&assessment)).unwrap();
    registry.unregister("demo").unwrap();
    let connection = database(&registry);
    connection
        .execute("UPDATE harness_results SET check_id = 'missing'", [])
        .unwrap();
    assert_eq!(
        registry.promotion_approvals(None).unwrap_err().code(),
        "storage"
    );
    connection
        .execute("UPDATE harness_results SET check_id = 'smoke'", [])
        .unwrap();
    connection
        .execute(
            "UPDATE consumer_deployments SET consumer_target = 'other'",
            [],
        )
        .unwrap();
    assert_eq!(
        registry.promotion_approvals(None).unwrap_err().code(),
        "storage"
    );
    connection
        .execute(
            "UPDATE consumer_deployments SET consumer_target = 'lab / applications'",
            [],
        )
        .unwrap();
    connection
        .execute("DELETE FROM harness_results", [])
        .unwrap();
    assert_eq!(
        registry.promotion_approvals(None).unwrap_err().code(),
        "storage"
    );
}

#[test]
fn corrupt_consumer_evidence_is_not_a_missing_optional_step_or_a_success_fallback() {
    let (fixture, registry, app) = setup(OPTIONAL);
    let context = context(&app);
    registry
        .record_consumer_deployment(&deployment(&context, "personal-device"))
        .unwrap();
    registry
        .record_consumer_deployment(&deployment(&context, "personal-device"))
        .unwrap();
    let before = registry.assess_promotion(&context).unwrap();
    let connection = database(&registry);
    connection
        .execute(
            "UPDATE consumer_deployments SET consumer_target = '' WHERE id = 2",
            [],
        )
        .unwrap();
    assert_eq!(
        registry.assess_promotion(&context).unwrap_err().code(),
        "storage"
    );
    assert_eq!(
        registry
            .approve_promotion(&approval(&before))
            .unwrap_err()
            .code(),
        "storage"
    );
    assert_eq!(
        Registry::open(&fixture.state).unwrap_err().code(),
        "storage"
    );
    assert_eq!(count(&connection, "promotion_approvals"), 0);
}

#[test]
fn every_promotion_schema_and_autoincrement_sequence_is_validated_before_use() {
    for sql in [
        "DROP TABLE harness_results",
        "DROP TABLE promotion_approvals",
        "DROP INDEX harness_results_app_id",
        "DROP INDEX promotion_approvals_app_id",
        "CREATE UNIQUE INDEX unsafe_result ON harness_results(release_ref)",
        "CREATE UNIQUE INDEX unsafe_approval ON promotion_approvals(app_id)",
        "CREATE TRIGGER unsafe_result AFTER INSERT ON harness_results BEGIN DELETE FROM harness_results; END",
        "CREATE TRIGGER unsafe_approval AFTER INSERT ON promotion_approvals BEGIN DELETE FROM promotion_approvals; END",
        "UPDATE sqlite_sequence SET seq = 'bad' WHERE name = 'harness_results'",
        "UPDATE sqlite_sequence SET seq = 0 WHERE name = 'promotion_approvals'",
        "INSERT INTO sqlite_sequence (name, seq) VALUES ('harness_results', 100)",
        "DELETE FROM sqlite_sequence WHERE name = 'promotion_approvals'",
    ] {
        let (fixture, registry, app) = setup(HARNESS);
        let context = context(&app);
        registry
            .record_harness_result(&report(&context, 50, 45))
            .unwrap();
        let assessment = registry.assess_promotion(&context).unwrap();
        registry.approve_promotion(&approval(&assessment)).unwrap();
        let connection = database(&registry);
        connection.execute_batch(sql).unwrap();
        assert_eq!(
            Registry::open(&fixture.state).unwrap_err().code(),
            "storage",
            "{sql}"
        );
        assert_eq!(
            registry.assess_promotion(&context).unwrap_err().code(),
            "storage",
            "{sql}"
        );
        assert_eq!(
            registry
                .record_harness_result(&report(&context, 50, 45))
                .unwrap_err()
                .code(),
            "storage",
            "{sql}"
        );
        assert_eq!(
            registry
                .approve_promotion(&approval(&assessment))
                .unwrap_err()
                .code(),
            "storage",
            "{sql}"
        );
        assert_eq!(
            registry.promotion_approvals(None).unwrap_err().code(),
            "storage",
            "{sql}"
        );
        assert_eq!(count(&connection, "registered_apps"), 1);
    }
}

#[test]
fn incompatible_new_history_table_definitions_are_not_accepted_or_repaired() {
    for (table, from, to) in [
        ("harness_results", " AUTOINCREMENT", ""),
        (
            "harness_results",
            "total INTEGER NOT NULL",
            "total REAL NOT NULL",
        ),
        ("harness_results", "notes TEXT", "notes TEXT NOT NULL"),
        (
            "harness_results",
            "app_id TEXT NOT NULL",
            "app_id TEXT NOT NULL REFERENCES registered_apps(app_id) ON DELETE CASCADE",
        ),
        ("promotion_approvals", " AUTOINCREMENT", ""),
        (
            "promotion_approvals",
            "assessment TEXT NOT NULL",
            "assessment TEXT",
        ),
        (
            "promotion_approvals",
            "skipped_optional TEXT NOT NULL",
            "skipped_optional TEXT NOT NULL DEFAULT '[]'",
        ),
        (
            "promotion_approvals",
            "recorded_at TEXT NOT NULL",
            "recorded_at TEXT NOT NULL, unknown TEXT",
        ),
    ] {
        let (fixture, registry, app) = setup(HARNESS);
        let connection = database(&registry);
        let original: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        let changed = original.replace(from, to);
        assert_ne!(original, changed);
        connection
            .execute_batch(&format!(
                "ALTER TABLE {table} RENAME TO saved_table; DROP INDEX {table}_app_id;"
            ))
            .unwrap();
        connection.execute_batch(&changed).unwrap();
        connection
            .execute_batch(&format!("CREATE INDEX {table}_app_id ON {table}(app_id);"))
            .unwrap();
        assert_eq!(
            Registry::open(&fixture.state).unwrap_err().code(),
            "storage",
            "{table}: {from}"
        );
        assert_eq!(
            registry
                .assess_promotion(&context(&app))
                .unwrap_err()
                .code(),
            "storage"
        );
        assert_eq!(count(&connection, "registered_apps"), 1);
    }
}

#[test]
fn concurrent_reports_append_distinct_immutable_ids_without_deduplication() {
    let (_fixture, registry, app) = setup(HARNESS);
    let request = report(&context(&app), 50, 45);
    let count = 8;
    let barrier = Arc::new(Barrier::new(count));
    let threads: Vec<_> = (0..count)
        .map(|_| {
            let registry = registry.clone();
            let barrier = barrier.clone();
            let request = request.clone();
            thread::spawn(move || {
                barrier.wait();
                registry.record_harness_result(&request)
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap().unwrap())
        .collect();
    assert_eq!(
        results
            .iter()
            .map(|result| &result.id)
            .collect::<BTreeSet<_>>()
            .len(),
        count
    );
    let saved = registry.harness_results(None).unwrap();
    assert_eq!(saved.len(), count);
    assert!(results.iter().all(|result| saved.contains(result)));
}
