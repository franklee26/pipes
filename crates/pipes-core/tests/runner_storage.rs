use std::fs;

use pipes_core::{RecordHarnessResult, Registry};
use rusqlite::Connection;

#[test]
fn upgrades_v5_offline_preserving_snapshots_and_external_evidence() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    let manifest = root.path().join("pipes.yaml");
    fs::write(&manifest, "api: pipes/v1\napp: test\nsource: {path: source}\nversion: {from: VERSION}\nstages:\n  - id: test\n    checks: [{id: suite, exec: [test-tool]}]\n").unwrap();
    let data = root.path().join("state");
    let registry = Registry::open(&data).unwrap();
    let registered = registry.register(&manifest, None, false).unwrap();
    let report = registry
        .record_harness_result(&RecordHarnessResult {
            app_id: "test".to_owned(),
            stage_id: "test".to_owned(),
            check_id: "suite".to_owned(),
            expected_manifest_hash: registered.manifest.manifest_hash.clone(),
            expected_updated_at: registered.updated_at.clone(),
            release_ref: "candidate".to_owned(),
            total: 3,
            passed: 2,
            notes: None,
        })
        .unwrap();
    let connection = Connection::open(data.join("registry.sqlite3")).unwrap();
    connection
        .execute_batch(
            "DROP TABLE pipeline_run_logs; DROP TABLE pipeline_runs; PRAGMA user_version=5;",
        )
        .unwrap();
    let before: String = connection
        .query_row("SELECT snapshot FROM registered_apps", [], |row| row.get(0))
        .unwrap();
    fs::remove_file(manifest).unwrap();
    fs::remove_dir(source).unwrap();
    let migrated = Registry::open(&data).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let after: String = connection
        .query_row("SELECT snapshot FROM registered_apps", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 6);
    assert_eq!(before, after);
    assert_eq!(migrated.get("test").unwrap(), registered);
    assert_eq!(migrated.harness_results(None).unwrap(), [report]);
    assert!(migrated.pipeline_runs(None).unwrap().is_empty());
}

#[test]
fn incompatible_runner_tables_abort_upgrade_without_resetting_history() {
    let root = tempfile::tempdir().unwrap();
    let registry = Registry::open(root.path()).unwrap();
    let connection = Connection::open(root.path().join("registry.sqlite3")).unwrap();
    connection
        .execute_batch(
            "DROP TABLE pipeline_run_logs;
         CREATE TABLE pipeline_run_logs (id INTEGER PRIMARY KEY, precious TEXT);
         INSERT INTO pipeline_run_logs VALUES (1, 'keep');
         PRAGMA user_version=5;",
        )
        .unwrap();
    assert_eq!(Registry::open(root.path()).unwrap_err().code(), "storage");
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 5);
    let value: String = connection
        .query_row("SELECT precious FROM pipeline_run_logs", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(value, "keep");
    assert_eq!(
        registry.pipeline_runs(None).unwrap_err().code(),
        "unsupported_schema"
    );
}
