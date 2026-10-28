mod support;

use std::{
    fs,
    path::Path,
    sync::{Arc, Barrier},
    thread,
};

use pipes_core::{
    CheckPlanNode, Parallelism, PromotionContext, PromotionStatus, RecordConsumerDeployment,
    Registry, preview_manifest,
};
use rusqlite::{Connection, params, types::Value as SqlValue};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::{FULL, Fixture, MINIMAL, canonical};

const REGISTERED_AT: &str = "2026-01-02T03:04:05.123456+00:00";
const UPDATED_AT: &str = "2026-02-03T04:05:06.987654321Z";
const V1_SCHEMA: &str = "
    CREATE TABLE registered_apps (
        app_id TEXT NOT NULL PRIMARY KEY,
        manifest_path TEXT NOT NULL UNIQUE,
        manifest_hash TEXT NOT NULL,
        snapshot TEXT NOT NULL,
        registered_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
    PRAGMA user_version = 1;
";

const V4_HISTORY_SCHEMA: &str = "
    CREATE TABLE consumer_deployments (
        id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
        app_id TEXT NOT NULL,
        app_name TEXT NOT NULL,
        stage_id TEXT NOT NULL,
        consumer_id TEXT NOT NULL,
        consumer_name TEXT NOT NULL,
        consumer_target TEXT NOT NULL,
        consumer_approval TEXT NOT NULL,
        release_ref TEXT NOT NULL,
        notes TEXT,
        manifest_hash TEXT NOT NULL,
        manifest_path TEXT NOT NULL,
        source_path TEXT NOT NULL,
        registration_updated_at TEXT NOT NULL,
        recorded_at TEXT NOT NULL
    );
    CREATE INDEX consumer_deployments_app_id ON consumer_deployments(app_id);
";

#[derive(Clone, Copy)]
enum LegacyManifest {
    Minimal,
    Full,
}

fn legacy_snapshot(
    fixture: &Fixture,
    path: &Path,
    app_id: &str,
    yaml: &str,
    kind: LegacyManifest,
) -> Value {
    let mut snapshot = json!({
        "appId": app_id,
        "name": app_id,
        "manifestPath": canonical(path),
        "sourcePath": canonical(&fixture.source),
        "manifestHash": format!("{:x}", Sha256::digest(yaml.as_bytes())),
        "manifestYaml": yaml,
        "versionFrom": "file:VERSION",
        "execution": {"os": null, "arch": null, "timeout": null},
        "stages": [{"id": "build", "checks": [], "requires": [], "requiresApproval": false}],
        "prepare": [],
        "outputs": []
    });
    if matches!(kind, LegacyManifest::Full) {
        snapshot["name"] = json!("Sample Application");
        snapshot["execution"] = json!({"os": "macos", "arch": "arm64", "timeout": "1h 30m"});
        snapshot["stages"] = json!([
            {
                "id": "tested",
                "checks": [{
                    "id": "unit", "exec": ["cargo", "test"], "cwd": null,
                    "timeout": null, "env": {}, "secrets": {}
                }],
                "requires": ["checks.passed", "prepare.version.passed"],
                "requiresApproval": false
            },
            {
                "id": "shipped", "checks": [],
                "requires": ["previous.passed", "approval.local"], "requiresApproval": true
            }
        ]);
        snapshot["prepare"] = json!([{
            "id": "version", "exec": ["tool-not-installed", "argument with spaces", ""],
            "cwd": "directory-not-created", "timeout": "10s",
            "env": {"BUILD_KIND": "release", "EMPTY": ""},
            "secrets": {"TOKEN": "env://PIPES_TEST_TOKEN", "ACCOUNT": "keychain://service account"}
        }]);
        snapshot["outputs"] = json!([
            {"name": "binary", "path": "dist/My App", "format": "file"},
            {"name": "bundle", "path": "dist/My App.app", "format": "directory"}
        ]);
    }
    snapshot
}

fn seed_v1(fixture: &Fixture, app_ids: &[&str], kind: LegacyManifest) -> (Connection, Vec<Value>) {
    fs::create_dir(&fixture.state).unwrap();
    let connection = Connection::open(fixture.state.join("registry.sqlite3")).unwrap();
    connection.execute_batch(V1_SCHEMA).unwrap();
    let mode: String = connection
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    let mut snapshots = Vec::new();
    for (index, app_id) in app_ids.iter().enumerate() {
        let (template, old_id) = match kind {
            LegacyManifest::Minimal => (MINIMAL, "demo"),
            LegacyManifest::Full => (FULL, "sample-app"),
        };
        let yaml = format!(
            "{}# preserved legacy bytes\r\n",
            template
                .replace(&format!("app: {old_id}"), &format!("app: {app_id}"))
                .replace('\n', "\r\n")
        );
        let path = if index == 0 {
            fixture.write(&yaml);
            fixture.manifest.clone()
        } else {
            let path = fixture.path(&format!("{app_id}.yaml"));
            fs::write(&path, &yaml).unwrap();
            path
        };
        let snapshot = legacy_snapshot(fixture, &path, app_id, &yaml, kind);
        connection
            .execute(
                "INSERT INTO registered_apps \
             (app_id, manifest_path, manifest_hash, snapshot, registered_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    app_id,
                    snapshot["manifestPath"].as_str().unwrap(),
                    snapshot["manifestHash"].as_str().unwrap(),
                    snapshot.to_string(),
                    REGISTERED_AT,
                    UPDATED_AT
                ],
            )
            .unwrap();
        snapshots.push(snapshot);
    }
    (connection, snapshots)
}

fn seed_v2(fixture: &Fixture, app_ids: &[&str]) -> (Connection, Vec<Value>) {
    let (connection, mut snapshots) = seed_v1(fixture, app_ids, LegacyManifest::Full);
    for snapshot in &mut snapshots {
        let old_yaml = snapshot["manifestYaml"].as_str().unwrap();
        let (prefix, stages_and_retention) = old_yaml.split_once("stages:\r\n").unwrap();
        let (_, retention) = stages_and_retention.split_once("retention:\r\n").unwrap();
        let stages = "stages:
  - id: shipped
    after: [lint, tested]
    require: [previous.passed, approval.local]
  - id: tested
    after: []
    checks:
      - id: unit
        exec: [cargo, test]
    require: [checks.passed, prepare.version.passed]
  - id: lint
    after: [tested]
    checks:
      - id: lint-check
        exec: [test-tool, lint]
    require: [checks.passed]
  - id: docs
    after: []
";
        let yaml = format!(
            "{prefix}{}retention:\r\n{retention}",
            stages.replace('\n', "\r\n")
        );
        let hash = format!("{:x}", Sha256::digest(yaml.as_bytes()));
        let mut tested = snapshot["stages"][0].clone();
        tested["after"] = json!([]);
        let mut shipped = snapshot["stages"][1].clone();
        shipped["after"] = json!(["lint", "tested"]);
        snapshot["stages"] = json!([
            shipped,
            tested,
            {
                "id": "lint", "after": ["tested"],
                "checks": [{
                    "id": "lint-check", "exec": ["test-tool", "lint"], "cwd": null,
                    "timeout": null, "env": {}, "secrets": {}
                }],
                "requires": ["checks.passed"], "requiresApproval": false
            },
            {"id": "docs", "after": [], "checks": [], "requires": [], "requiresApproval": false}
        ]);
        fs::write(snapshot["manifestPath"].as_str().unwrap(), &yaml).unwrap();
        snapshot["manifestYaml"] = json!(yaml);
        snapshot["manifestHash"] = json!(hash);
        connection
            .execute(
                "UPDATE registered_apps SET snapshot = ?1, manifest_hash = ?2 WHERE app_id = ?3",
                params![
                    snapshot.to_string(),
                    hash,
                    snapshot["appId"].as_str().unwrap()
                ],
            )
            .unwrap();
    }
    connection.pragma_update(None, "user_version", 2).unwrap();
    (connection, snapshots)
}

fn seed_v3(fixture: &Fixture, app_ids: &[&str]) -> (Connection, Vec<Value>) {
    let (connection, mut snapshots) = seed_v2(fixture, app_ids);
    for snapshot in &mut snapshots {
        let yaml = snapshot["manifestYaml"].as_str().unwrap()
            .replace("execution:\r\n", "execution:\r\n  max_parallel: 4\r\n")
            .replace(
                "    checks:\r\n      - id: unit\r\n        exec: [cargo, test]\r\n",
                "    checks:\r\n      - id: batch\r\n        max_parallel: 2\r\n        parallel:\r\n          - id: ordered\r\n            sequence:\r\n              - id: unit\r\n                exec: [cargo, test]\r\n",
            );
        snapshot["execution"]["maxParallel"] = json!(4);
        for stage in snapshot["stages"].as_array_mut().unwrap() {
            stage["checkPlan"] = json!(
                stage["checks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|command| { json!({"kind": "command", "command": command}) })
                    .collect::<Vec<_>>()
            );
        }
        let command = snapshot["stages"][1]["checks"][0].clone();
        snapshot["stages"][1]["checkPlan"] = json!([{
            "kind": "parallel", "id": "batch", "maxParallel": 2,
            "steps": [{
                "kind": "sequence", "id": "ordered",
                "steps": [{"kind": "command", "command": command}]
            }]
        }]);
        let hash = format!("{:x}", Sha256::digest(yaml.as_bytes()));
        fs::write(snapshot["manifestPath"].as_str().unwrap(), &yaml).unwrap();
        snapshot["manifestYaml"] = json!(yaml);
        snapshot["manifestHash"] = json!(hash);
        connection
            .execute(
                "UPDATE registered_apps SET snapshot = ?1, manifest_hash = ?2 WHERE app_id = ?3",
                params![
                    snapshot.to_string(),
                    hash,
                    snapshot["appId"].as_str().unwrap()
                ],
            )
            .unwrap();
    }
    connection.pragma_update(None, "user_version", 3).unwrap();
    (connection, snapshots)
}

fn seed_v4(fixture: &Fixture, app_ids: &[&str]) -> (Connection, Vec<Value>) {
    let (connection, mut snapshots) = seed_v3(fixture, app_ids);
    connection.execute_batch(V4_HISTORY_SCHEMA).unwrap();
    for (index, snapshot) in snapshots.iter_mut().enumerate() {
        let yaml = snapshot["manifestYaml"].as_str().unwrap().replace(
            "  - id: tested\r\n",
            "  - id: tested\r\n    consumers: [{id: laptop, name: Lab Laptop, target: 'Lab / Apps', approval: manual}]\r\n",
        );
        for stage in snapshot["stages"].as_array_mut().unwrap() {
            stage["consumers"] = json!([]);
        }
        snapshot["stages"][1]["consumers"] = json!([{
            "id": "laptop", "name": "Lab Laptop", "target": "Lab / Apps", "approval": "manual"
        }]);
        let hash = format!("{:x}", Sha256::digest(yaml.as_bytes()));
        fs::write(snapshot["manifestPath"].as_str().unwrap(), &yaml).unwrap();
        snapshot["manifestYaml"] = json!(yaml);
        snapshot["manifestHash"] = json!(hash);
        connection
            .execute(
                "UPDATE registered_apps SET snapshot = ?1, manifest_hash = ?2 WHERE app_id = ?3",
                params![
                    snapshot.to_string(),
                    hash,
                    snapshot["appId"].as_str().unwrap()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO consumer_deployments \
             (id, app_id, app_name, stage_id, consumer_id, consumer_name, consumer_target, \
              consumer_approval, release_ref, notes, manifest_hash, manifest_path, source_path, \
              registration_updated_at, recorded_at) \
             VALUES (?1, ?2, ?3, 'tested', 'laptop', 'Lab Laptop', 'Lab / Apps', 'manual', \
                     'legacy-release', 'Historical manual approval', ?4, ?5, ?6, ?7, ?8)",
                params![
                    (index as i64 + 1) * 7,
                    snapshot["appId"].as_str().unwrap(),
                    snapshot["name"].as_str().unwrap(),
                    snapshot["manifestHash"].as_str().unwrap(),
                    snapshot["manifestPath"].as_str().unwrap(),
                    snapshot["sourcePath"].as_str().unwrap(),
                    UPDATED_AT,
                    "2026-03-04T05:06:07.123456789Z",
                ],
            )
            .unwrap();
    }
    if let Some(snapshot) = snapshots.first() {
        connection.execute(
            "INSERT INTO consumer_deployments \
             (id, app_id, app_name, stage_id, consumer_id, consumer_name, consumer_target, \
              consumer_approval, release_ref, notes, manifest_hash, manifest_path, source_path, \
              registration_updated_at, recorded_at) \
             VALUES (9007199254740993, 'archived-app', 'Archived App', 'retired', 'old-device', \
                     'Old Device', 'Old / Apps', 'manual', 'archived-release', NULL, ?1, ?2, ?3, ?4, ?5)",
            params![
                snapshot["manifestHash"].as_str().unwrap(),
                snapshot["manifestPath"].as_str().unwrap(),
                snapshot["sourcePath"].as_str().unwrap(),
                UPDATED_AT, UPDATED_AT,
            ],
        ).unwrap();
        connection.execute(
            "UPDATE sqlite_sequence SET seq = 9007199254741993 WHERE name = 'consumer_deployments'",
            [],
        ).unwrap();
    }
    connection.pragma_update(None, "user_version", 4).unwrap();
    (connection, snapshots)
}

#[derive(Debug, PartialEq, Eq)]
struct SavedRow {
    app_id: String,
    manifest_path: String,
    manifest_hash: String,
    snapshot: String,
    registered_at: String,
    updated_at: String,
}

fn rows(connection: &Connection) -> Vec<SavedRow> {
    let mut statement = connection
        .prepare(
            "SELECT app_id, manifest_path, manifest_hash, snapshot, registered_at, updated_at \
         FROM registered_apps ORDER BY app_id COLLATE BINARY",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            Ok(SavedRow {
                app_id: row.get(0)?,
                manifest_path: row.get(1)?,
                manifest_hash: row.get(2)?,
                snapshot: row.get(3)?,
                registered_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn version(connection: &Connection) -> i64 {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

fn assert_no_history_schema(connection: &Connection) {
    let objects: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'consumer_deployments%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(objects, 0);
}

#[test]
fn migrates_frozen_v1_snapshots_offline_preserving_every_existing_field() {
    let fixture = Fixture::new(FULL);
    let (connection, originals) = seed_v1(&fixture, &["alpha", "zulu"], LegacyManifest::Full);
    let before = rows(&connection);
    for snapshot in &originals {
        fs::remove_file(snapshot["manifestPath"].as_str().unwrap()).unwrap();
    }
    fs::remove_dir(&fixture.source).unwrap();

    let registry = Registry::open(&fixture.state).unwrap();
    assert_eq!(version(&connection), 6);
    let apps = registry.list().unwrap();
    assert_eq!(apps.len(), originals.len());
    for (app, original) in apps.iter().zip(&originals) {
        assert!(app.manifest.stages[0].after.is_empty());
        assert_eq!(app.manifest.stages[1].after, ["tested"]);
        assert_eq!(app.registered_at, REGISTERED_AT);
        assert_eq!(app.updated_at, UPDATED_AT);
        assert_eq!(app.manifest.execution.max_parallel, Parallelism::Auto);
        for stage in &app.manifest.stages {
            assert!(stage.consumers.is_empty());
            assert_eq!(
                stage.check_plan,
                stage
                    .checks
                    .iter()
                    .cloned()
                    .map(|command| CheckPlanNode::Command { command })
                    .collect::<Vec<_>>()
            );
        }
        let mut upgraded = serde_json::to_value(&app.manifest).unwrap();
        for stage in upgraded["stages"].as_array_mut().unwrap() {
            stage.as_object_mut().unwrap().remove("after");
            stage.as_object_mut().unwrap().remove("checkPlan");
            stage.as_object_mut().unwrap().remove("consumers");
            assert_eq!(stage["promotionCriteria"], json!([]));
            stage.as_object_mut().unwrap().remove("promotionCriteria");
        }
        upgraded["execution"]
            .as_object_mut()
            .unwrap()
            .remove("maxParallel");
        assert_eq!(&upgraded, original);
        assert_eq!(registry.get(&app.manifest.app_id).unwrap(), *app);
    }
    let after = rows(&connection);
    for (before, after) in before.iter().zip(&after) {
        assert_eq!(after.app_id, before.app_id);
        assert_eq!(after.manifest_path, before.manifest_path);
        assert_eq!(after.manifest_hash, before.manifest_hash);
        assert_eq!(after.registered_at, before.registered_at);
        assert_eq!(after.updated_at, before.updated_at);
        assert_ne!(after.snapshot, before.snapshot);
    }
    assert_eq!(
        Registry::open(&fixture.state).unwrap().list().unwrap(),
        apps
    );
    assert_eq!(rows(&connection), after);
}

#[test]
fn migrated_minimum_snapshot_is_idempotent_and_can_be_explicitly_replaced_by_a_graph() {
    let fixture = Fixture::new(MINIMAL);
    let (connection, originals) = seed_v1(&fixture, &["demo"], LegacyManifest::Minimal);
    let registry = Registry::open(&fixture.state).unwrap();
    let migrated = registry.get("demo").unwrap();
    assert_eq!(migrated.manifest.stages.len(), 1);
    assert!(migrated.manifest.stages[0].after.is_empty());
    assert_eq!(
        migrated.manifest.manifest_hash,
        originals[0]["manifestHash"]
    );
    assert_eq!(
        registry.register(&fixture.manifest, None, false).unwrap(),
        migrated
    );
    let graph = MINIMAL.replace(
        "[{id: build}]",
        "[{id: build}, {id: test}, {id: lint, after: [build]}, {id: release, after: [lint, test]}]",
    );
    fixture.write(&graph);
    let preview = preview_manifest(&fixture.manifest).unwrap();
    assert_eq!(
        registry
            .register(&fixture.manifest, None, false)
            .unwrap_err()
            .code(),
        "already_registered"
    );
    let updated = registry
        .register(&fixture.manifest, Some(&preview.manifest_hash), true)
        .unwrap();
    assert_eq!(updated.manifest, preview);
    assert_eq!(updated.manifest.stages[3].after, ["lint", "test"]);
    assert_eq!(updated.registered_at, REGISTERED_AT);
    assert_eq!(version(&connection), 6);
}

#[test]
fn empty_v1_registry_is_upgraded_without_creating_registrations() {
    let fixture = Fixture::new(MINIMAL);
    let (connection, _) = seed_v1(&fixture, &[], LegacyManifest::Minimal);
    let registry = Registry::open(&fixture.state).unwrap();
    assert_eq!(version(&connection), 6);
    assert!(registry.list().unwrap().is_empty());
    assert!(registry.consumer_deployments(None).unwrap().is_empty());
}

#[test]
fn corrupt_legacy_snapshots_roll_back_all_rows_and_the_schema_version() {
    let fixture = Fixture::new(FULL);
    let (connection, originals) = seed_v1(&fixture, &["alpha", "zulu"], LegacyManifest::Full);
    let original = &originals[1];
    let mut corruptions = vec!["{".to_owned()];
    for (field, value) in [
        ("name", json!("not the saved YAML name")),
        ("manifestHash", json!("bad-hash")),
        ("manifestYaml", json!("broken: [")),
        ("sourcePath", json!("relative")),
        ("unknown", json!(true)),
    ] {
        let mut snapshot = original.clone();
        snapshot[field] = value;
        corruptions.push(snapshot.to_string());
    }
    let mut snapshot = original.clone();
    snapshot["execution"]["maxParallel"] = json!("auto");
    corruptions.push(snapshot.to_string());
    let mut snapshot = original.clone();
    snapshot["stages"][0]["checkPlan"] = json!([]);
    corruptions.push(snapshot.to_string());
    let mut snapshot = original.clone();
    snapshot["stages"][0]["after"] = json!([]);
    corruptions.push(snapshot.to_string());
    let mut snapshot = original.clone();
    snapshot["stages"][0]
        .as_object_mut()
        .unwrap()
        .remove("checks");
    corruptions.push(snapshot.to_string());
    let mut snapshot = original.clone();
    snapshot["stages"].as_array_mut().unwrap().reverse();
    corruptions.push(snapshot.to_string());
    let mut snapshot = original.clone();
    snapshot["stages"][1]["requiresApproval"] = json!(false);
    corruptions.push(snapshot.to_string());
    for snapshot in corruptions {
        connection
            .execute(
                "UPDATE registered_apps SET snapshot = ?1 WHERE app_id = 'zulu'",
                [snapshot],
            )
            .unwrap();
        let before = rows(&connection);
        let error = Registry::open(&fixture.state).unwrap_err();
        assert_eq!(error.code(), "storage", "{error}");
        assert!(error.to_string().contains("zulu"), "{error}");
        assert_eq!(version(&connection), 1);
        assert_eq!(rows(&connection), before);
        assert_no_history_schema(&connection);
        let alpha: Value = serde_json::from_str(&before[0].snapshot).unwrap();
        assert!(alpha["stages"][0].get("after").is_none());
    }
}

#[test]
fn invalid_legacy_metadata_also_rolls_back_preceding_valid_rows() {
    let fixture = Fixture::new(MINIMAL);
    let (connection, _) = seed_v1(&fixture, &["alpha", "zulu"], LegacyManifest::Minimal);
    for timestamp in ["invalid", "2026-01-01T12:00:00+01:00"] {
        connection
            .execute(
                "UPDATE registered_apps SET updated_at = ?1 WHERE app_id = 'zulu'",
                [timestamp],
            )
            .unwrap();
        let before = rows(&connection);
        assert_eq!(
            Registry::open(&fixture.state).unwrap_err().code(),
            "storage"
        );
        assert_eq!(version(&connection), 1);
        assert_eq!(rows(&connection), before);
    }
    connection
        .execute(
            "UPDATE registered_apps SET updated_at = ?1, manifest_hash = ?2 WHERE app_id = 'zulu'",
            params![UPDATED_AT, "wrong-indexed-hash"],
        )
        .unwrap();
    let before = rows(&connection);
    assert_eq!(
        Registry::open(&fixture.state).unwrap_err().code(),
        "storage"
    );
    assert_eq!(version(&connection), 1);
    assert_eq!(rows(&connection), before);
}

#[test]
fn v1_yaml_must_pass_the_original_schema_even_when_new_graph_fields_are_valid() {
    let fixture = Fixture::new(FULL);
    let (connection, originals) = seed_v1(&fixture, &["alpha", "zulu"], LegacyManifest::Full);
    let mut snapshot = originals[1].clone();
    let yaml = snapshot["manifestYaml"]
        .as_str()
        .unwrap()
        .replace("  - id: tested\r\n", "  - id: tested\r\n    after: []\r\n");
    let hash = format!("{:x}", Sha256::digest(yaml.as_bytes()));
    snapshot["manifestYaml"] = json!(yaml);
    snapshot["manifestHash"] = json!(hash);
    connection
        .execute(
            "UPDATE registered_apps SET snapshot = ?1, manifest_hash = ?2 WHERE app_id = 'zulu'",
            params![snapshot.to_string(), hash],
        )
        .unwrap();
    let before = rows(&connection);
    let error = Registry::open(&fixture.state).unwrap_err();
    assert_eq!(error.code(), "storage");
    assert!(error.to_string().contains("schema-v1 snapshots"), "{error}");
    assert_eq!(version(&connection), 1);
    assert_eq!(rows(&connection), before);
}

#[test]
fn legacy_schema_constraints_are_validated_before_any_snapshot_updates() {
    let fixture = Fixture::new(MINIMAL);
    let (connection, _) = seed_v1(&fixture, &["demo"], LegacyManifest::Minimal);
    connection
        .execute_batch(
            "ALTER TABLE registered_apps RENAME TO saved_v1;
         CREATE TABLE registered_apps (
             app_id TEXT NOT NULL PRIMARY KEY,
             manifest_path TEXT NOT NULL,
             manifest_hash TEXT NOT NULL,
             snapshot TEXT NOT NULL,
             registered_at TEXT NOT NULL,
             updated_at TEXT NOT NULL
         );
         INSERT INTO registered_apps SELECT * FROM saved_v1;
         DROP TABLE saved_v1;",
        )
        .unwrap();
    let before = rows(&connection);
    let error = Registry::open(&fixture.state).unwrap_err();
    assert_eq!(error.code(), "storage");
    assert!(error.to_string().contains("uniqueness"), "{error}");
    assert_eq!(version(&connection), 1);
    assert_eq!(rows(&connection), before);
}

#[test]
fn unexpected_legacy_triggers_are_rejected_without_upgrading_snapshots() {
    let fixture = Fixture::new(MINIMAL);
    let (connection, _) = seed_v1(&fixture, &["alpha", "zulu"], LegacyManifest::Minimal);
    connection
        .execute_batch(
            "CREATE TRIGGER fail_second_upgrade BEFORE UPDATE OF snapshot ON registered_apps
         WHEN OLD.app_id = 'zulu'
         BEGIN SELECT RAISE(ABORT, 'injected migration write failure'); END;",
        )
        .unwrap();
    let before = rows(&connection);
    let error = Registry::open(&fixture.state).unwrap_err();
    assert_eq!(error.code(), "storage");
    assert!(error.to_string().contains("unexpected trigger"), "{error}");
    assert_eq!(version(&connection), 1);
    assert_eq!(rows(&connection), before);
}

#[test]
fn concurrent_old_schema_opens_upgrade_once_without_changing_saved_metadata() {
    let fixture = Fixture::new(FULL);
    let (connection, _) = seed_v1(&fixture, &["alpha", "zulu"], LegacyManifest::Full);
    fs::remove_file(&fixture.manifest).unwrap();
    fs::remove_file(fixture.path("zulu.yaml")).unwrap();
    fs::remove_dir(&fixture.source).unwrap();
    let count = 6;
    let barrier = Arc::new(Barrier::new(count));
    let threads: Vec<_> = (0..count)
        .map(|_| {
            let state = fixture.state.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                Registry::open(state)?.list()
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap().unwrap())
        .collect();
    assert!(results.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(version(&connection), 6);
    for app in &results[0] {
        assert_eq!(app.registered_at, REGISTERED_AT);
        assert_eq!(app.updated_at, UPDATED_AT);
        assert_eq!(app.manifest.stages[1].after, ["tested"]);
    }
}

#[test]
fn concurrent_failed_migrations_leave_the_entire_v1_registry_unchanged() {
    let fixture = Fixture::new(MINIMAL);
    let (connection, _) = seed_v1(&fixture, &["alpha", "zulu"], LegacyManifest::Minimal);
    connection
        .execute(
            "UPDATE registered_apps SET snapshot = '{' WHERE app_id = 'zulu'",
            [],
        )
        .unwrap();
    let before = rows(&connection);
    let count = 4;
    let barrier = Arc::new(Barrier::new(count));
    let threads: Vec<_> = (0..count)
        .map(|_| {
            let state = fixture.state.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                Registry::open(state).unwrap_err().code()
            })
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().unwrap(), "storage");
    }
    assert_eq!(version(&connection), 1);
    assert_eq!(rows(&connection), before);
}

#[test]
fn v2_records_without_after_are_corrupt_and_live_handles_do_not_auto_migrate() {
    let fixture = Fixture::new(MINIMAL);
    let (connection, snapshots) = seed_v1(&fixture, &["demo"], LegacyManifest::Minimal);
    let registry = Registry::open(&fixture.state).unwrap();
    connection
        .execute(
            "UPDATE registered_apps SET snapshot = ?1 WHERE app_id = 'demo'",
            [snapshots[0].to_string()],
        )
        .unwrap();
    assert_eq!(version(&connection), 6);
    assert_eq!(registry.get("demo").unwrap_err().code(), "storage");
    connection.pragma_update(None, "user_version", 2).unwrap();
    assert_eq!(
        Registry::open(&fixture.state).unwrap_err().code(),
        "storage"
    );
    connection.pragma_update(None, "user_version", 1).unwrap();
    assert_eq!(registry.list().unwrap_err().code(), "unsupported_schema");
    assert_eq!(
        registry
            .register(&fixture.manifest, None, false)
            .unwrap_err()
            .code(),
        "unsupported_schema"
    );
    assert_eq!(
        registry.unregister("demo").unwrap_err().code(),
        "unsupported_schema"
    );
    assert_eq!(version(&connection), 1);
    connection
        .execute_batch(
            "DROP TABLE consumer_deployments;
             DROP TABLE harness_results;
             DROP TABLE promotion_approvals;
             DROP TABLE pipeline_run_logs;
             DROP TABLE pipeline_runs;",
        )
        .unwrap();
    assert!(
        Registry::open(&fixture.state)
            .unwrap()
            .get("demo")
            .unwrap()
            .manifest
            .stages[0]
            .after
            .is_empty()
    );
    assert_eq!(version(&connection), 6);
}

#[test]
fn v2_graph_snapshots_upgrade_offline_preserving_all_metadata_edges_and_checks() {
    let fixture = Fixture::new(FULL);
    let (connection, originals) = seed_v2(&fixture, &["alpha", "zulu"]);
    let before = rows(&connection);
    for original in &originals {
        fs::remove_file(original["manifestPath"].as_str().unwrap()).unwrap();
    }
    fs::remove_dir(&fixture.source).unwrap();
    let registry = Registry::open(&fixture.state).unwrap();
    assert_eq!(version(&connection), 6);
    let apps = registry.list().unwrap();
    for (app, original) in apps.iter().zip(&originals) {
        assert_eq!(app.manifest.execution.max_parallel, Parallelism::Auto);
        assert_eq!(app.registered_at, REGISTERED_AT);
        assert_eq!(app.updated_at, UPDATED_AT);
        assert_eq!(app.manifest.stages[0].id, "shipped");
        assert_eq!(app.manifest.stages[0].after, ["lint", "tested"]);
        for stage in &app.manifest.stages {
            assert!(stage.consumers.is_empty());
            assert_eq!(
                stage.check_plan,
                stage
                    .checks
                    .iter()
                    .cloned()
                    .map(|command| CheckPlanNode::Command { command })
                    .collect::<Vec<_>>()
            );
        }
        let mut restored = serde_json::to_value(&app.manifest).unwrap();
        restored["execution"]
            .as_object_mut()
            .unwrap()
            .remove("maxParallel");
        for stage in restored["stages"].as_array_mut().unwrap() {
            stage.as_object_mut().unwrap().remove("checkPlan");
            stage.as_object_mut().unwrap().remove("consumers");
            assert_eq!(stage["promotionCriteria"], json!([]));
            stage.as_object_mut().unwrap().remove("promotionCriteria");
        }
        assert_eq!(&restored, original);
    }
    let after = rows(&connection);
    for (before, after) in before.iter().zip(&after) {
        assert_eq!(after.app_id, before.app_id);
        assert_eq!(after.manifest_path, before.manifest_path);
        assert_eq!(after.manifest_hash, before.manifest_hash);
        assert_eq!(after.registered_at, before.registered_at);
        assert_eq!(after.updated_at, before.updated_at);
    }
    assert_eq!(
        Registry::open(&fixture.state).unwrap().list().unwrap(),
        apps
    );
    assert_eq!(rows(&connection), after);
}

#[test]
fn corrupt_v2_snapshots_roll_back_every_row_and_keep_version_two() {
    let fixture = Fixture::new(FULL);
    let (connection, originals) = seed_v2(&fixture, &["alpha", "zulu"]);
    let original = &originals[1];
    let mut corruptions = vec!["{".to_owned()];
    let mut value = original.clone();
    value["execution"]["maxParallel"] = json!("auto");
    corruptions.push(value.to_string());
    let mut value = original.clone();
    value["stages"][1]["checkPlan"] = json!([]);
    corruptions.push(value.to_string());
    let mut value = original.clone();
    value["stages"][0].as_object_mut().unwrap().remove("after");
    corruptions.push(value.to_string());
    let mut value = original.clone();
    value["stages"][1]["checks"][0]["exec"] = json!(["different-command"]);
    corruptions.push(value.to_string());
    let mut value = original.clone();
    value["stages"][0]["after"] = json!(["tested", "lint"]);
    corruptions.push(value.to_string());
    for snapshot in corruptions {
        connection
            .execute(
                "UPDATE registered_apps SET snapshot = ?1 WHERE app_id = 'zulu'",
                [snapshot],
            )
            .unwrap();
        let before = rows(&connection);
        let error = Registry::open(&fixture.state).unwrap_err();
        assert_eq!(error.code(), "storage");
        assert!(error.to_string().contains("zulu"), "{error}");
        assert_eq!(version(&connection), 2);
        assert_eq!(rows(&connection), before);
        assert_no_history_schema(&connection);
        let alpha: Value = serde_json::from_str(&before[0].snapshot).unwrap();
        assert!(alpha["execution"].get("maxParallel").is_none());
        assert!(alpha["stages"][0].get("checkPlan").is_none());
    }
}

#[test]
fn both_legacy_versions_reject_new_yaml_grammar_even_when_the_flattened_commands_match() {
    for old_version in [1, 2] {
        let fixture = Fixture::new(FULL);
        let (connection, originals) = if old_version == 1 {
            seed_v1(&fixture, &["alpha", "zulu"], LegacyManifest::Full)
        } else {
            seed_v2(&fixture, &["alpha", "zulu"])
        };
        for original in &originals {
            fs::remove_file(original["manifestPath"].as_str().unwrap()).unwrap();
        }
        fs::remove_dir(&fixture.source).unwrap();
        let original = &originals[1];
        for (from, to, reason) in [
            (
                "execution:\r\n",
                "execution:\r\n  max_parallel: auto\r\n",
                "max_parallel",
            ),
            (
                "    checks:\r\n      - id: unit\r\n        exec: [cargo, test]\r\n",
                "    checks:\r\n      - id: batch\r\n        parallel:\r\n          - id: unit\r\n            exec: [cargo, test]\r\n",
                "check groups",
            ),
        ] {
            let mut value = original.clone();
            let yaml = value["manifestYaml"].as_str().unwrap().replace(from, to);
            assert_ne!(yaml, value["manifestYaml"].as_str().unwrap());
            let hash = format!("{:x}", Sha256::digest(yaml.as_bytes()));
            value["manifestYaml"] = json!(yaml);
            value["manifestHash"] = json!(hash);
            connection.execute(
                "UPDATE registered_apps SET snapshot = ?1, manifest_hash = ?2 WHERE app_id = 'zulu'",
                params![value.to_string(), hash],
            ).unwrap();
            let before = rows(&connection);
            let error = Registry::open(&fixture.state).unwrap_err();
            assert_eq!(error.code(), "storage");
            assert!(error.to_string().contains(reason), "{error}");
            assert!(
                error.to_string().contains("schema-v1/v2 snapshots"),
                "{error}"
            );
            assert_eq!(version(&connection), old_version);
            assert_eq!(rows(&connection), before);
        }
    }
}

#[test]
fn v2_migration_write_failures_and_invalid_constraints_do_not_partially_upgrade() {
    let fixture = Fixture::new(FULL);
    let (connection, _) = seed_v2(&fixture, &["alpha", "zulu"]);
    connection
        .execute_batch(
            "CREATE TRIGGER fail_second_upgrade BEFORE UPDATE OF snapshot ON registered_apps
         WHEN OLD.app_id = 'zulu'
         BEGIN SELECT RAISE(ABORT, 'injected v2 migration failure'); END;",
        )
        .unwrap();
    let before = rows(&connection);
    let error = Registry::open(&fixture.state).unwrap_err();
    assert_eq!(error.code(), "storage");
    assert!(error.to_string().contains("unexpected trigger"), "{error}");
    assert_eq!(version(&connection), 2);
    assert_eq!(rows(&connection), before);
    connection
        .execute_batch(
            "DROP TRIGGER fail_second_upgrade;
         ALTER TABLE registered_apps RENAME TO saved_v2;
         CREATE TABLE registered_apps (
             app_id TEXT NOT NULL PRIMARY KEY,
             manifest_path TEXT NOT NULL,
             manifest_hash TEXT NOT NULL,
             snapshot TEXT NOT NULL,
             registered_at TEXT NOT NULL,
             updated_at TEXT NOT NULL
         );
         INSERT INTO registered_apps SELECT * FROM saved_v2;
         DROP TABLE saved_v2;",
        )
        .unwrap();
    let error = Registry::open(&fixture.state).unwrap_err();
    assert_eq!(error.code(), "storage");
    assert!(error.to_string().contains("uniqueness"), "{error}");
    assert_eq!(version(&connection), 2);
    assert_eq!(rows(&connection), before);
}

#[test]
fn concurrent_v2_opens_publish_only_complete_current_snapshots() {
    let fixture = Fixture::new(FULL);
    let (connection, originals) = seed_v2(&fixture, &["alpha", "zulu"]);
    for original in originals {
        fs::remove_file(original["manifestPath"].as_str().unwrap()).unwrap();
    }
    fs::remove_dir(&fixture.source).unwrap();
    let count = 6;
    let barrier = Arc::new(Barrier::new(count));
    let threads: Vec<_> = (0..count)
        .map(|_| {
            let state = fixture.state.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                Registry::open(state)?.list()
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap().unwrap())
        .collect();
    assert!(results.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(version(&connection), 6);
    for app in &results[0] {
        assert_eq!(app.manifest.execution.max_parallel, Parallelism::Auto);
        assert_eq!(app.manifest.stages[0].after, ["lint", "tested"]);
        assert_eq!(app.manifest.stages[1].check_plan.len(), 1);
        assert_eq!(app.registered_at, REGISTERED_AT);
        assert_eq!(app.updated_at, UPDATED_AT);
    }
}

#[test]
fn v2_upgrades_are_idempotent_and_plan_replacement_stays_explicit() {
    let fixture = Fixture::new(FULL);
    let (connection, originals) = seed_v2(&fixture, &["demo"]);
    let registry = Registry::open(&fixture.state).unwrap();
    let original = registry.get("demo").unwrap();
    assert_eq!(
        registry.register(&fixture.manifest, None, false).unwrap(),
        original
    );
    let yaml = originals[0]["manifestYaml"].as_str().unwrap()
        .replace("execution:\r\n", "execution:\r\n  max_parallel: 2\r\n")
        .replace(
            "    checks:\r\n      - id: unit\r\n        exec: [cargo, test]\r\n",
            "    checks:\r\n      - id: batch\r\n        parallel:\r\n          - id: unit\r\n            exec: [cargo, test]\r\n",
        );
    fixture.write(&yaml);
    assert_eq!(
        registry
            .register(&fixture.manifest, None, false)
            .unwrap_err()
            .code(),
        "already_registered"
    );
    assert_eq!(
        registry
            .register(
                &fixture.manifest,
                Some(&original.manifest.manifest_hash),
                true
            )
            .unwrap_err()
            .code(),
        "manifest_changed"
    );
    let preview = preview_manifest(&fixture.manifest).unwrap();
    let updated = registry
        .register(&fixture.manifest, Some(&preview.manifest_hash), true)
        .unwrap();
    assert_eq!(updated.manifest, preview);
    assert_eq!(updated.registered_at, REGISTERED_AT);
    assert!(matches!(
        &updated.manifest.stages[1].check_plan[0],
        CheckPlanNode::Parallel { .. }
    ));
    assert_eq!(version(&connection), 6);
}

#[test]
fn v3_migrates_offline_preserving_plans_budgets_edges_and_all_provenance() {
    let fixture = Fixture::new(FULL);
    let (connection, originals) = seed_v3(&fixture, &["alpha", "zulu"]);
    let before = rows(&connection);
    for original in &originals {
        fs::remove_file(original["manifestPath"].as_str().unwrap()).unwrap();
    }
    fs::remove_dir(&fixture.source).unwrap();
    let registry = Registry::open(&fixture.state).unwrap();
    assert_eq!(version(&connection), 6);
    assert!(registry.consumer_deployments(None).unwrap().is_empty());
    let apps = registry.list().unwrap();
    assert_eq!(apps.len(), originals.len());
    for (app, original) in apps.iter().zip(&originals) {
        assert_eq!(app.registered_at, REGISTERED_AT);
        assert_eq!(app.updated_at, UPDATED_AT);
        assert_eq!(app.manifest.stages[0].after, ["lint", "tested"]);
        assert_eq!(
            serde_json::to_value(app.manifest.execution.max_parallel).unwrap(),
            json!(4)
        );
        let mut upgraded = serde_json::to_value(&app.manifest).unwrap();
        for stage in upgraded["stages"].as_array_mut().unwrap() {
            assert_eq!(stage["consumers"], json!([]));
            stage.as_object_mut().unwrap().remove("consumers");
            assert_eq!(stage["promotionCriteria"], json!([]));
            stage.as_object_mut().unwrap().remove("promotionCriteria");
        }
        assert_eq!(&upgraded, original);
    }
    let after = rows(&connection);
    for (before, after) in before.iter().zip(&after) {
        assert_eq!(before.app_id, after.app_id);
        assert_eq!(before.manifest_path, after.manifest_path);
        assert_eq!(before.manifest_hash, after.manifest_hash);
        assert_eq!(before.registered_at, after.registered_at);
        assert_eq!(before.updated_at, after.updated_at);
        assert_ne!(before.snapshot, after.snapshot);
    }
    assert_eq!(
        Registry::open(&fixture.state).unwrap().list().unwrap(),
        apps
    );
    assert_eq!(rows(&connection), after);
}

#[test]
fn v3_corrupt_snapshots_roll_back_every_row_and_history_schema_creation() {
    let fixture = Fixture::new(FULL);
    let (connection, originals) = seed_v3(&fixture, &["alpha", "zulu"]);
    for original in &originals {
        fs::remove_file(original["manifestPath"].as_str().unwrap()).unwrap();
    }
    fs::remove_dir(&fixture.source).unwrap();
    let original = &originals[1];
    let mut corruptions = vec!["{".to_owned()];
    for field in ["checkPlan", "after", "checks"] {
        let mut value = original.clone();
        value["stages"][1].as_object_mut().unwrap().remove(field);
        corruptions.push(value.to_string());
    }
    let mut value = original.clone();
    value["execution"]
        .as_object_mut()
        .unwrap()
        .remove("maxParallel");
    corruptions.push(value.to_string());
    for cap in [json!("auto"), json!(0), json!(2)] {
        let mut value = original.clone();
        value["execution"]["maxParallel"] = cap;
        corruptions.push(value.to_string());
    }
    let mut value = original.clone();
    value["stages"][1]["checkPlan"][0]["maxParallel"] = json!(4);
    corruptions.push(value.to_string());
    let mut value = original.clone();
    value["stages"][1]["checkPlan"] = json!([{
        "kind": "command", "command": value["stages"][1]["checks"][0]
    }]);
    corruptions.push(value.to_string());
    let mut value = original.clone();
    value["stages"][0]["after"] = json!(["tested", "lint"]);
    corruptions.push(value.to_string());
    for consumers in [json!([]), Value::Null] {
        let mut value = original.clone();
        value["stages"][0]["consumers"] = consumers;
        corruptions.push(value.to_string());
    }
    for (key, replacement) in [
        ("manifestHash", json!("bad")),
        ("manifestYaml", json!("broken: [")),
        ("sourcePath", json!("relative")),
        ("name", json!("Forged name")),
    ] {
        let mut value = original.clone();
        value[key] = replacement;
        corruptions.push(value.to_string());
    }
    for snapshot in corruptions {
        connection
            .execute(
                "UPDATE registered_apps SET snapshot = ?1 WHERE app_id = 'zulu'",
                [snapshot],
            )
            .unwrap();
        let before = rows(&connection);
        let error = Registry::open(&fixture.state).unwrap_err();
        assert_eq!(error.code(), "storage", "{error}");
        assert!(error.to_string().contains("zulu"), "{error}");
        assert_eq!(version(&connection), 3);
        assert_eq!(rows(&connection), before);
        assert_no_history_schema(&connection);
    }
}

#[test]
fn every_legacy_version_rejects_consumer_yaml_and_json_even_when_consumers_are_empty() {
    for old_version in [1, 2, 3] {
        let fixture = Fixture::new(FULL);
        let (connection, originals) = match old_version {
            1 => seed_v1(&fixture, &["alpha", "zulu"], LegacyManifest::Full),
            2 => seed_v2(&fixture, &["alpha", "zulu"]),
            _ => seed_v3(&fixture, &["alpha", "zulu"]),
        };
        for original in &originals {
            fs::remove_file(original["manifestPath"].as_str().unwrap()).unwrap();
        }
        fs::remove_dir(&fixture.source).unwrap();
        let original = &originals[1];
        for consumers in ["[]", "[{id: mac, target: Applications}]"] {
            let mut value = original.clone();
            let yaml = value["manifestYaml"].as_str().unwrap().replace(
                "  - id: tested\r\n",
                &format!("  - id: tested\r\n    consumers: {consumers}\r\n"),
            );
            let hash = format!("{:x}", Sha256::digest(yaml.as_bytes()));
            value["manifestYaml"] = json!(yaml);
            value["manifestHash"] = json!(hash);
            connection.execute(
                "UPDATE registered_apps SET snapshot = ?1, manifest_hash = ?2 WHERE app_id = 'zulu'",
                params![value.to_string(), hash],
            ).unwrap();
            let before = rows(&connection);
            let error = Registry::open(&fixture.state).unwrap_err();
            assert_eq!(error.code(), "storage");
            assert!(
                error.to_string().contains("schema-v1/v2/v3 snapshots"),
                "{error}"
            );
            assert_eq!(version(&connection), old_version);
            assert_eq!(rows(&connection), before);
            assert_no_history_schema(&connection);
        }
        let mut value = original.clone();
        value["stages"][0]["consumers"] = json!([]);
        connection.execute(
            "UPDATE registered_apps SET snapshot = ?1, manifest_hash = ?2 WHERE app_id = 'zulu'",
            params![value.to_string(), value["manifestHash"].as_str().unwrap()],
        ).unwrap();
        let before = rows(&connection);
        let error = Registry::open(&fixture.state).unwrap_err();
        assert_eq!(error.code(), "storage");
        assert!(error.to_string().contains("consumers"), "{error}");
        assert_eq!(rows(&connection), before);
        assert_eq!(version(&connection), old_version);
        assert_no_history_schema(&connection);
    }
}

#[test]
fn all_legacy_versions_refuse_existing_history_schema_without_overwriting_anything() {
    for old_version in [1, 2, 3] {
        let fixture = Fixture::new(FULL);
        let (connection, _) = match old_version {
            1 => seed_v1(&fixture, &["alpha", "zulu"], LegacyManifest::Full),
            2 => seed_v2(&fixture, &["alpha", "zulu"]),
            _ => seed_v3(&fixture, &["alpha", "zulu"]),
        };
        connection
            .execute_batch(
                "CREATE TABLE consumer_deployments (keep TEXT);
             INSERT INTO consumer_deployments VALUES ('preserve me');",
            )
            .unwrap();
        let before = rows(&connection);
        assert_eq!(
            Registry::open(&fixture.state).unwrap_err().code(),
            "storage"
        );
        assert_eq!(version(&connection), old_version);
        assert_eq!(rows(&connection), before);
        let saved: String = connection
            .query_row("SELECT keep FROM consumer_deployments", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(saved, "preserve me");
    }
}

fn history_rows(connection: &Connection) -> Vec<Vec<SqlValue>> {
    let mut statement = connection
        .prepare(
            "SELECT id, app_id, app_name, stage_id, consumer_id, consumer_name, consumer_target, \
         consumer_approval, release_ref, notes, manifest_hash, manifest_path, source_path, \
         registration_updated_at, recorded_at FROM consumer_deployments ORDER BY id",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            (0..15)
                .map(|index| row.get(index))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn sequences(connection: &Connection) -> Vec<(String, SqlValue)> {
    connection
        .prepare("SELECT name, seq FROM sqlite_sequence ORDER BY name")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn assert_no_promotion_schema(connection: &Connection) {
    let count: i64 = connection.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name LIKE 'harness_results%' OR name LIKE 'promotion_approvals%'",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn v4_migrates_offline_preserving_all_consumer_history_ids_sequences_and_snapshot_fields() {
    let fixture = Fixture::new(FULL);
    let (connection, originals) = seed_v4(&fixture, &["alpha", "zulu"]);
    let before = rows(&connection);
    let history = history_rows(&connection);
    let sequence = sequences(&connection);
    let table_root: i64 = connection
        .query_row(
            "SELECT rootpage FROM sqlite_master WHERE name = 'consumer_deployments'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    for original in &originals {
        fs::remove_file(original["manifestPath"].as_str().unwrap()).unwrap();
    }
    fs::remove_dir(&fixture.source).unwrap();
    let registry = Registry::open(&fixture.state).unwrap();
    assert_eq!(version(&connection), 6);
    assert_eq!(history_rows(&connection), history);
    assert_eq!(sequences(&connection), sequence);
    let migrated_root: i64 = connection
        .query_row(
            "SELECT rootpage FROM sqlite_master WHERE name = 'consumer_deployments'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_root, migrated_root);
    let apps = registry.list().unwrap();
    for (app, original) in apps.iter().zip(&originals) {
        assert_eq!(app.registered_at, REGISTERED_AT);
        assert_eq!(app.updated_at, UPDATED_AT);
        let mut current = serde_json::to_value(&app.manifest).unwrap();
        for stage in current["stages"].as_array_mut().unwrap() {
            assert_eq!(stage["promotionCriteria"], json!([]));
            stage.as_object_mut().unwrap().remove("promotionCriteria");
        }
        assert_eq!(&current, original);
        let assessment = registry
            .assess_promotion(&PromotionContext {
                app_id: app.manifest.app_id.clone(),
                stage_id: "tested".to_owned(),
                expected_manifest_hash: app.manifest.manifest_hash.clone(),
                expected_updated_at: app.updated_at.clone(),
                release_ref: "legacy-release".to_owned(),
            })
            .unwrap();
        assert_eq!(assessment.status, PromotionStatus::NotConfigured);
    }
    for (old, current) in before.iter().zip(rows(&connection)) {
        assert_eq!(old.app_id, current.app_id);
        assert_eq!(old.manifest_path, current.manifest_path);
        assert_eq!(old.manifest_hash, current.manifest_hash);
        assert_eq!(old.registered_at, current.registered_at);
        assert_eq!(old.updated_at, current.updated_at);
    }
    assert_eq!(registry.consumer_deployments(None).unwrap().len(), 3);
    assert_eq!(
        registry.consumer_deployments(Some("archived-app")).unwrap()[0].id,
        "9007199254740993"
    );
    assert!(registry.harness_results(None).unwrap().is_empty());
    assert!(registry.promotion_approvals(None).unwrap().is_empty());
    let app = &apps[0];
    let marker = registry
        .record_consumer_deployment(&RecordConsumerDeployment {
            app_id: app.manifest.app_id.clone(),
            stage_id: "tested".to_owned(),
            consumer_id: "laptop".to_owned(),
            expected_manifest_hash: app.manifest.manifest_hash.clone(),
            expected_updated_at: app.updated_at.clone(),
            release_ref: "new-release".to_owned(),
            notes: None,
            approved: true,
        })
        .unwrap();
    assert_eq!(marker.id, "9007199254741994");
    assert_eq!(registry.get(&app.manifest.app_id).unwrap(), *app);
    let after = rows(&connection);
    let reopened = Registry::open(&fixture.state).unwrap();
    assert_eq!(reopened.list().unwrap(), apps);
    assert_eq!(rows(&connection), after);
    assert_eq!(reopened.consumer_deployments(None).unwrap().len(), 4);
}

#[test]
fn v4_bad_snapshots_history_and_schema_roll_back_migration_atomically() {
    for corrupt in [
        "UPDATE registered_apps SET snapshot = '{' WHERE app_id = 'zulu'",
        "UPDATE registered_apps SET updated_at = 'bad' WHERE app_id = 'zulu'",
        "UPDATE consumer_deployments SET consumer_target = '' WHERE app_id = 'archived-app'",
        "UPDATE consumer_deployments SET consumer_approval = 'auto' WHERE app_id = 'zulu'",
        "UPDATE consumer_deployments SET recorded_at = 'bad' WHERE app_id = 'zulu'",
        "UPDATE consumer_deployments SET notes = ' padded ' WHERE app_id = 'zulu'",
        "UPDATE consumer_deployments SET release_ref = '' WHERE app_id = 'zulu'",
        "UPDATE consumer_deployments SET source_path = 'relative' WHERE app_id = 'zulu'",
        "UPDATE consumer_deployments SET id = 0 WHERE app_id = 'zulu'",
        "UPDATE sqlite_sequence SET seq = 'bad' WHERE name = 'consumer_deployments'",
        "DROP INDEX consumer_deployments_app_id",
        "CREATE TRIGGER unsafe_history AFTER INSERT ON consumer_deployments BEGIN DELETE FROM consumer_deployments; END",
        "CREATE TRIGGER unsafe_registration AFTER UPDATE ON registered_apps BEGIN DELETE FROM consumer_deployments; END",
    ] {
        let fixture = Fixture::new(FULL);
        let (connection, originals) = seed_v4(&fixture, &["alpha", "zulu"]);
        for original in &originals {
            fs::remove_file(original["manifestPath"].as_str().unwrap()).unwrap();
        }
        fs::remove_dir(&fixture.source).unwrap();
        connection.execute_batch(corrupt).unwrap();
        let registrations = rows(&connection);
        let history = history_rows(&connection);
        let sequence = sequences(&connection);
        let error = Registry::open(&fixture.state).unwrap_err();
        assert_eq!(error.code(), "storage", "{corrupt}: {error}");
        assert_eq!(version(&connection), 4);
        assert_eq!(rows(&connection), registrations);
        assert_eq!(history_rows(&connection), history);
        assert_eq!(sequences(&connection), sequence);
        assert_no_promotion_schema(&connection);
        let alpha: Value = serde_json::from_str(&registrations[0].snapshot).unwrap();
        assert!(alpha["stages"][0].get("promotionCriteria").is_none());
    }
}

#[test]
fn every_legacy_version_rejects_modern_promotion_yaml_and_json_even_when_explicitly_empty() {
    for old_version in [1, 2, 3, 4] {
        let fixture = Fixture::new(FULL);
        let (connection, originals) = match old_version {
            1 => seed_v1(&fixture, &["alpha", "zulu"], LegacyManifest::Full),
            2 => seed_v2(&fixture, &["alpha", "zulu"]),
            3 => seed_v3(&fixture, &["alpha", "zulu"]),
            _ => seed_v4(&fixture, &["alpha", "zulu"]),
        };
        for original in &originals {
            fs::remove_file(original["manifestPath"].as_str().unwrap()).unwrap();
        }
        fs::remove_dir(&fixture.source).unwrap();
        let mut corruptions = Vec::new();
        for policy in [
            "null",
            "{}",
            "{criteria: []}",
            "{criteria: [{id: suite, kind: harness, check: unit, expected_tests: 50, minimum_passed: 45}]}",
        ] {
            let mut value = originals[1].clone();
            let yaml = value["manifestYaml"].as_str().unwrap().replace(
                "  - id: tested\r\n",
                &format!("  - id: tested\r\n    promotion: {policy}\r\n"),
            );
            value["manifestHash"] = json!(format!("{:x}", Sha256::digest(yaml.as_bytes())));
            value["manifestYaml"] = json!(yaml);
            corruptions.push(value);
        }
        for criteria in [
            json!([]),
            Value::Null,
            json!([{"id": "suite", "kind": "harness", "checkId": "unit", "expectedTests": 50, "minimumPassed": 45}]),
        ] {
            let mut value = originals[1].clone();
            value["stages"][0]["promotionCriteria"] = criteria;
            corruptions.push(value);
        }
        for value in corruptions {
            connection.execute(
                "UPDATE registered_apps SET snapshot = ?1, manifest_hash = ?2 WHERE app_id = 'zulu'",
                params![value.to_string(), value["manifestHash"].as_str().unwrap()],
            ).unwrap();
            let before = rows(&connection);
            let history = (old_version == 4).then(|| history_rows(&connection));
            let sequence = (old_version == 4).then(|| sequences(&connection));
            let error = Registry::open(&fixture.state).unwrap_err();
            assert_eq!(error.code(), "storage", "v{old_version}: {error}");
            assert_eq!(version(&connection), old_version);
            assert_eq!(rows(&connection), before);
            assert_no_promotion_schema(&connection);
            if old_version == 4 {
                assert_eq!(Some(history_rows(&connection)), history);
                assert_eq!(Some(sequences(&connection)), sequence);
            } else {
                assert_no_history_schema(&connection);
            }
        }
    }
}

#[test]
fn every_legacy_version_refuses_preexisting_promotion_tables_without_overwriting_them() {
    for old_version in [1, 2, 3, 4] {
        let fixture = Fixture::new(FULL);
        let (connection, _) = match old_version {
            1 => seed_v1(&fixture, &["alpha", "zulu"], LegacyManifest::Full),
            2 => seed_v2(&fixture, &["alpha", "zulu"]),
            3 => seed_v3(&fixture, &["alpha", "zulu"]),
            _ => seed_v4(&fixture, &["alpha", "zulu"]),
        };
        connection.execute_batch("CREATE TABLE harness_results (keep TEXT); INSERT INTO harness_results VALUES ('preserve me');").unwrap();
        let before = rows(&connection);
        assert_eq!(
            Registry::open(&fixture.state).unwrap_err().code(),
            "storage"
        );
        assert_eq!(version(&connection), old_version);
        assert_eq!(rows(&connection), before);
        let saved: String = connection
            .query_row("SELECT keep FROM harness_results", [], |row| row.get(0))
            .unwrap();
        assert_eq!(saved, "preserve me");
    }
}

#[test]
fn concurrent_v4_migrations_preserve_frozen_history_and_upgrade_only_once() {
    let fixture = Fixture::new(FULL);
    let (connection, originals) = seed_v4(&fixture, &["alpha", "zulu"]);
    let history = history_rows(&connection);
    let sequence = sequences(&connection);
    for original in &originals {
        fs::remove_file(original["manifestPath"].as_str().unwrap()).unwrap();
    }
    fs::remove_dir(&fixture.source).unwrap();
    let count = 6;
    let barrier = Arc::new(Barrier::new(count));
    let threads: Vec<_> = (0..count)
        .map(|_| {
            let state = fixture.state.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                let registry = Registry::open(state)?;
                Ok::<_, pipes_core::Error>((registry.list()?, registry.consumer_deployments(None)?))
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap().unwrap())
        .collect();
    assert!(results.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(version(&connection), 6);
    assert_eq!(history_rows(&connection), history);
    assert_eq!(sequences(&connection), sequence);
}
