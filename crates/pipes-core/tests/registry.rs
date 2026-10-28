mod support;

use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

use chrono::DateTime;
use pipes_core::{RegisteredApp, Registry, preview_manifest};
use rusqlite::{Connection, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use support::{FULL, Fixture, MINIMAL, canonical};

fn database(registry: &Registry) -> Connection {
    Connection::open(registry.data_dir().join("registry.sqlite3")).unwrap()
}

#[test]
fn snapshots_survive_reopening_and_removal_of_original_yaml_and_source() {
    let fixture = Fixture::new(FULL);
    let registry = Registry::open(&fixture.state).unwrap();
    let registered = registry.register(&fixture.manifest, None, false).unwrap();
    assert_eq!(
        registry.data_dir().to_str().unwrap(),
        canonical(&fixture.state)
    );
    for value in [&registered.registered_at, &registered.updated_at] {
        assert_eq!(
            DateTime::parse_from_rfc3339(value)
                .unwrap()
                .offset()
                .local_minus_utc(),
            0
        );
    }
    assert_eq!(registered.registered_at, registered.updated_at);
    drop(registry);
    fs::remove_file(&fixture.manifest).unwrap();
    fs::remove_dir(&fixture.source).unwrap();
    let reopened = Registry::open(&fixture.state).unwrap();
    assert_eq!(reopened.get("sample-app").unwrap(), registered);
    assert_eq!(
        reopened.list().unwrap().as_slice(),
        std::slice::from_ref(&registered)
    );
    let encoded = serde_json::to_value(&registered).unwrap();
    assert!(encoded.get("registeredAt").is_some());
    assert!(encoded.get("updatedAt").is_some());
    assert_eq!(
        serde_json::from_value::<RegisteredApp>(encoded).unwrap(),
        registered
    );
}

#[test]
fn exact_registration_is_idempotent_including_both_timestamps() {
    let fixture = Fixture::new(MINIMAL);
    let registry = Registry::open(&fixture.state).unwrap();
    let first = registry.register(&fixture.manifest, None, false).unwrap();
    let second = Registry::open(&fixture.state)
        .unwrap()
        .register(
            &fixture.manifest,
            Some(&first.manifest.manifest_hash),
            false,
        )
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(
        registry.register(&fixture.manifest, None, true).unwrap(),
        first
    );
    assert_eq!(registry.list().unwrap().len(), 1);
}

#[test]
fn changed_bytes_require_explicit_replace_and_keep_original_registration_time() {
    let fixture = Fixture::new(MINIMAL);
    let registry = Registry::open(&fixture.state).unwrap();
    let first = registry.register(&fixture.manifest, None, false).unwrap();
    fixture.write(&format!("{MINIMAL}name: A new display name\n"));
    let error = registry
        .register(&fixture.manifest, None, false)
        .unwrap_err();
    assert_eq!(error.code(), "already_registered");
    assert!(error.to_string().contains("--replace"));
    assert_eq!(registry.get("demo").unwrap(), first);
    let updated = registry.register(&fixture.manifest, None, true).unwrap();
    assert_eq!(updated.registered_at, first.registered_at);
    assert_ne!(updated.updated_at, first.updated_at);
    assert_eq!(updated.manifest.name, "A new display name");
    assert_ne!(updated.manifest.manifest_hash, first.manifest.manifest_hash);
    assert_eq!(
        Registry::open(&fixture.state).unwrap().get("demo").unwrap(),
        updated
    );
}

#[test]
fn another_path_for_an_existing_app_id_also_needs_explicit_replacement() {
    let fixture = Fixture::new(MINIMAL);
    let registry = Registry::open(&fixture.state).unwrap();
    let first = registry.register(&fixture.manifest, None, false).unwrap();
    let other = fixture.path("other.yaml");
    fs::write(&other, MINIMAL).unwrap();
    assert_eq!(
        registry.register(&other, None, false).unwrap_err().code(),
        "already_registered"
    );
    let replaced = registry.register(&other, None, true).unwrap();
    assert_eq!(
        replaced.manifest.manifest_hash,
        first.manifest.manifest_hash
    );
    assert_eq!(replaced.manifest.manifest_path, canonical(&other));
    assert_eq!(replaced.registered_at, first.registered_at);
    fixture.write(&MINIMAL.replace("app: demo", "app: now-available"));
    registry.register(&fixture.manifest, None, false).unwrap();
    assert_eq!(registry.list().unwrap().len(), 2);
}

#[test]
fn path_collisions_cannot_implicitly_rename_app_identity_even_with_replace() {
    let fixture = Fixture::new(MINIMAL);
    let registry = Registry::open(&fixture.state).unwrap();
    let first = registry.register(&fixture.manifest, None, false).unwrap();
    fixture.write(&MINIMAL.replace("app: demo", "app: renamed"));
    for replace in [false, true] {
        let error = registry
            .register(&fixture.manifest, None, replace)
            .unwrap_err();
        assert_eq!(error.code(), "already_registered");
        assert!(error.to_string().contains("unregister"));
        assert_eq!(
            registry.list().unwrap().as_slice(),
            std::slice::from_ref(&first)
        );
    }
    registry.unregister("demo").unwrap();
    assert_eq!(
        registry
            .register(&fixture.manifest, None, false)
            .unwrap()
            .manifest
            .app_id,
        "renamed"
    );
}

#[cfg(unix)]
#[test]
fn canonical_manifest_aliases_are_the_same_registration_path() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new(MINIMAL);
    let registry = Registry::open(&fixture.state).unwrap();
    let original = registry.register(&fixture.manifest, None, false).unwrap();
    let alias = fixture.path("alias.yaml");
    symlink(&fixture.manifest, &alias).unwrap();
    assert_eq!(registry.register(&alias, None, false).unwrap(), original);
}

#[test]
fn expected_hash_detects_drift_including_now_invalid_yaml_without_replacing() {
    let fixture = Fixture::new(MINIMAL);
    let registry = Registry::open(&fixture.state).unwrap();
    let preview = preview_manifest(&fixture.manifest).unwrap();
    fixture.write(&format!("{MINIMAL}# changed after preview\n"));
    let error = registry
        .register(&fixture.manifest, Some(&preview.manifest_hash), false)
        .unwrap_err();
    assert_eq!(error.code(), "manifest_changed");
    assert!(registry.list().unwrap().is_empty());
    fixture.write(MINIMAL);
    let registered = registry
        .register(&fixture.manifest, Some(&preview.manifest_hash), false)
        .unwrap();
    fixture.write("not valid: [");
    assert_eq!(
        registry
            .register(&fixture.manifest, Some(&preview.manifest_hash), true)
            .unwrap_err()
            .code(),
        "manifest_changed"
    );
    assert_eq!(registry.get("demo").unwrap(), registered);
}

#[test]
fn unregister_only_removes_metadata_and_missing_ids_are_errors() {
    let fixture = Fixture::new(MINIMAL);
    let payload = fixture.source.join("keep-me.txt");
    fs::write(&payload, "untouched").unwrap();
    let registry = Registry::open(&fixture.state).unwrap();
    registry.register(&fixture.manifest, None, false).unwrap();
    registry.unregister("demo").unwrap();
    assert!(registry.list().unwrap().is_empty());
    assert_eq!(fs::read_to_string(&payload).unwrap(), "untouched");
    assert_eq!(fs::read_to_string(&fixture.manifest).unwrap(), MINIMAL);
    assert!(fixture.source.is_dir());
    assert!(fixture.state.join("registry.sqlite3").is_file());
    assert_eq!(registry.get("demo").unwrap_err().code(), "not_found");
    assert_eq!(registry.unregister("demo").unwrap_err().code(), "not_found");
}

#[test]
fn listing_is_stably_sorted_and_ids_are_bound_sql_parameters() {
    let fixture = Fixture::new(MINIMAL);
    let registry = Registry::open(&fixture.state).unwrap();
    for app in ["zebra", "alpha", "middle"] {
        let manifest = fixture.path(&format!("{app}.yaml"));
        fs::write(
            &manifest,
            MINIMAL.replace("app: demo", &format!("app: {app}")),
        )
        .unwrap();
        registry.register(&manifest, None, false).unwrap();
    }
    let ids: Vec<_> = registry
        .list()
        .unwrap()
        .into_iter()
        .map(|app| app.manifest.app_id)
        .collect();
    assert_eq!(ids, ["alpha", "middle", "zebra"]);
    for id in [
        "missing",
        "' OR 1=1 --",
        "alpha'; DELETE FROM registered_apps; --",
    ] {
        assert_eq!(registry.get(id).unwrap_err().code(), "not_found");
        assert_eq!(registry.unregister(id).unwrap_err().code(), "not_found");
    }
    assert_eq!(registry.list().unwrap().len(), 3);
}

#[test]
fn newer_schema_is_rejected_without_resetting_data_on_new_and_existing_handles() {
    let fixture = Fixture::new(MINIMAL);
    let registry = Registry::open(&fixture.state).unwrap();
    registry.register(&fixture.manifest, None, false).unwrap();
    let connection = database(&registry);
    connection.pragma_update(None, "user_version", 7).unwrap();
    assert_eq!(
        Registry::open(&fixture.state).unwrap_err().code(),
        "unsupported_schema"
    );
    assert_eq!(
        registry.get("demo").unwrap_err().code(),
        "unsupported_schema"
    );
    assert_eq!(
        registry.unregister("demo").unwrap_err().code(),
        "unsupported_schema"
    );
    let count: i64 = connection
        .query_row("SELECT count(*) FROM registered_apps", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 7);
}

#[test]
fn unversioned_occupied_databases_and_incompatible_schemas_are_not_reset() {
    let fixture = Fixture::new(MINIMAL);
    fs::create_dir(&fixture.state).unwrap();
    let connection = Connection::open(fixture.state.join("registry.sqlite3")).unwrap();
    connection
        .execute_batch("CREATE TABLE precious (value TEXT); INSERT INTO precious VALUES ('keep');")
        .unwrap();
    assert_eq!(
        Registry::open(&fixture.state).unwrap_err().code(),
        "storage"
    );
    connection.pragma_update(None, "user_version", 1).unwrap();
    assert_eq!(
        Registry::open(&fixture.state).unwrap_err().code(),
        "storage"
    );
    let value: String = connection
        .query_row("SELECT value FROM precious", [], |row| row.get(0))
        .unwrap();
    assert_eq!(value, "keep");
}

#[test]
fn missing_uniqueness_constraints_are_detected_before_writing() {
    let fixture = Fixture::new(MINIMAL);
    let registry = Registry::open(&fixture.state).unwrap();
    database(&registry)
        .execute_batch(
            "DROP TABLE registered_apps;
         CREATE TABLE registered_apps (
             app_id TEXT NOT NULL PRIMARY KEY,
             manifest_path TEXT NOT NULL,
             manifest_hash TEXT NOT NULL,
             snapshot TEXT NOT NULL,
             registered_at TEXT NOT NULL,
             updated_at TEXT NOT NULL
         );",
        )
        .unwrap();
    let error = registry
        .register(&fixture.manifest, None, false)
        .unwrap_err();
    assert_eq!(error.code(), "storage");
    assert!(error.to_string().contains("uniqueness"));
}

#[test]
fn corrupt_database_file_and_deleted_live_database_are_errors() {
    let fixture = Fixture::new(MINIMAL);
    fs::create_dir(&fixture.state).unwrap();
    let path = fixture.state.join("registry.sqlite3");
    fs::write(&path, "this is not a SQLite database").unwrap();
    assert_eq!(
        Registry::open(&fixture.state).unwrap_err().code(),
        "storage"
    );
    fs::remove_file(&path).unwrap();
    let registry = Registry::open(&fixture.state).unwrap();
    fs::remove_file(&path).unwrap();
    assert_eq!(registry.list().unwrap_err().code(), "storage");
    assert!(!path.exists());
}

#[test]
fn corrupt_snapshot_json_is_never_treated_as_empty_missing_or_successful() {
    let fixture = Fixture::new(MINIMAL);
    let registry = Registry::open(&fixture.state).unwrap();
    registry.register(&fixture.manifest, None, false).unwrap();
    database(&registry)
        .execute("UPDATE registered_apps SET snapshot = ?1", ["{"])
        .unwrap();
    for error in [
        registry.get("demo").unwrap_err(),
        registry.list().unwrap_err(),
        registry
            .register(&fixture.manifest, None, false)
            .unwrap_err(),
        registry.unregister("demo").unwrap_err(),
    ] {
        assert_eq!(error.code(), "storage");
        assert!(error.to_string().contains("demo"));
    }
    let count: i64 = database(&registry)
        .query_row("SELECT count(*) FROM registered_apps", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn valid_json_with_corrupt_fields_hashes_or_timestamps_is_rejected() {
    let fixture = Fixture::new(MINIMAL);
    let registry = Registry::open(&fixture.state).unwrap();
    let registered = registry.register(&fixture.manifest, None, false).unwrap();
    let original = serde_json::to_value(&registered.manifest).unwrap();
    let connection = database(&registry);
    for (key, value) in [
        ("appId", Value::String("other".to_owned())),
        ("name", Value::String("inconsistent".to_owned())),
        ("manifestHash", Value::String("bad-hash".to_owned())),
        ("manifestYaml", Value::String("broken: [".to_owned())),
        ("sourcePath", Value::String("relative".to_owned())),
        ("stages", Value::Array(vec![])),
    ] {
        let mut corrupt = original.clone();
        corrupt[key] = value;
        connection
            .execute(
                "UPDATE registered_apps SET snapshot = ?1",
                [corrupt.to_string()],
            )
            .unwrap();
        assert_eq!(registry.get("demo").unwrap_err().code(), "storage", "{key}");
    }
    connection
        .execute(
            "UPDATE registered_apps SET snapshot = ?1",
            [original.to_string()],
        )
        .unwrap();
    for timestamp in ["not a timestamp", "2026-01-01T12:00:00+01:00"] {
        connection
            .execute("UPDATE registered_apps SET updated_at = ?1", [timestamp])
            .unwrap();
        assert_eq!(registry.list().unwrap_err().code(), "storage");
    }
    connection
        .execute(
            "UPDATE registered_apps SET updated_at = ?1, manifest_hash = ?2",
            params![registered.updated_at, "wrong-indexed-hash"],
        )
        .unwrap();
    assert_eq!(registry.list().unwrap_err().code(), "storage");
}

#[test]
fn independent_concurrent_instances_cannot_overwrite_competing_registrations() {
    let fixture = Fixture::new(MINIMAL);
    let count = 6;
    let barrier = Arc::new(Barrier::new(count));
    let threads: Vec<_> = (0..count)
        .map(|index| {
            let path = fixture.path(&format!("contender-{index}.yaml"));
            fs::write(&path, format!("{MINIMAL}name: Contender {index}\n")).unwrap();
            let state = fixture.state.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                Registry::open(state)?.register(path, None, false)
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(
        results.iter().filter(|result| result.is_ok()).count(),
        1,
        "{results:?}"
    );
    for error in results.iter().filter_map(|result| result.as_ref().err()) {
        assert_eq!(error.code(), "already_registered", "{error}");
    }
    let registry = Registry::open(&fixture.state).unwrap();
    assert_eq!(registry.list().unwrap().len(), 1);
    let winner = results.into_iter().find_map(|result| result.ok()).unwrap();
    assert_eq!(registry.get("demo").unwrap(), winner);
}

#[test]
fn concurrent_identical_registration_is_idempotent_across_instances() {
    let fixture = Fixture::new(MINIMAL);
    let count = 6;
    let barrier = Arc::new(Barrier::new(count));
    let threads: Vec<_> = (0..count)
        .map(|_| {
            let path = fixture.manifest.clone();
            let state = fixture.state.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                Registry::open(state)?.register(path, None, false)
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap().unwrap())
        .collect();
    assert!(results.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn wal_and_schema_version_are_persisted() {
    let fixture = Fixture::new(MINIMAL);
    let registry = Registry::open(&fixture.state).unwrap();
    let connection = database(&registry);
    let mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    assert_eq!(version, 6);
}

#[test]
fn invalid_data_directories_are_actionable_errors() {
    let fixture = Fixture::new(MINIMAL);
    assert_eq!(Registry::open("").unwrap_err().code(), "invalid_data_dir");
    assert_eq!(
        Registry::open(&fixture.manifest).unwrap_err().code(),
        "storage"
    );
    fs::create_dir_all(fixture.state.join("registry.sqlite3")).unwrap();
    assert_eq!(
        Registry::open(&fixture.state).unwrap_err().code(),
        "storage"
    );
}

#[cfg(unix)]
#[test]
fn new_state_is_private_but_existing_directory_permissions_are_unchanged() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new(MINIMAL);
    Registry::open(&fixture.state).unwrap();
    assert_eq!(
        fs::metadata(&fixture.state).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(fixture.state.join("registry.sqlite3"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let existing = fixture.path("existing");
    fs::create_dir(&existing).unwrap();
    fs::set_permissions(&existing, fs::Permissions::from_mode(0o755)).unwrap();
    Registry::open(&existing).unwrap();
    assert_eq!(
        fs::metadata(&existing).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[cfg(unix)]
#[test]
fn symlinked_database_is_rejected_without_modifying_its_target() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new(MINIMAL);
    fs::create_dir(&fixture.state).unwrap();
    symlink(&fixture.manifest, fixture.state.join("registry.sqlite3")).unwrap();
    assert_eq!(
        Registry::open(&fixture.state).unwrap_err().code(),
        "storage"
    );
    assert_eq!(fs::read_to_string(&fixture.manifest).unwrap(), MINIMAL);
}

#[cfg(unix)]
#[test]
fn live_handles_do_not_follow_replaced_database_symlinks() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new(MINIMAL);
    let registry = Registry::open(&fixture.state).unwrap();
    let target = fixture.path("do-not-change.sqlite3");
    let database_path = fixture.state.join("registry.sqlite3");
    fs::rename(&database_path, &target).unwrap();
    let original = fs::read(&target).unwrap();
    symlink(&target, &database_path).unwrap();
    assert_eq!(registry.list().unwrap_err().code(), "storage");
    assert_eq!(
        registry
            .register(&fixture.manifest, None, false)
            .unwrap_err()
            .code(),
        "storage"
    );
    assert_eq!(fs::read(&target).unwrap(), original);
}

#[test]
fn graph_snapshots_round_trip_without_their_original_files() {
    let yaml = MINIMAL.replace(
        "[{id: build}]",
        "[{id: release, after: [lint, test], require: [previous.passed]}, \
          {id: build, after: []}, {id: test, after: [build]}, {id: lint, after: [build]}, \
          {id: docs, after: []}]",
    );
    let fixture = Fixture::new(&yaml);
    let registry = Registry::open(&fixture.state).unwrap();
    let preview = preview_manifest(&fixture.manifest).unwrap();
    let registered = registry
        .register(&fixture.manifest, Some(&preview.manifest_hash), false)
        .unwrap();
    assert_eq!(registered.manifest, preview);
    assert_eq!(registered.manifest.stages[0].after, ["lint", "test"]);
    assert_eq!(
        registry.register(&fixture.manifest, None, false).unwrap(),
        registered
    );
    fs::remove_file(&fixture.manifest).unwrap();
    fs::remove_dir(&fixture.source).unwrap();
    let reopened = Registry::open(&fixture.state).unwrap();
    assert_eq!(reopened.get("demo").unwrap(), registered);
    assert_eq!(
        reopened.list().unwrap().as_slice(),
        std::slice::from_ref(&registered)
    );
    reopened.unregister("demo").unwrap();
}

#[test]
fn graph_snapshot_edges_are_required_and_checked_against_the_yaml() {
    let fixture = Fixture::new(&MINIMAL.replace(
        "[{id: build}]",
        "[{id: build}, {id: test}, {id: lint, after: [build]}, {id: release, after: [lint, test]}]",
    ));
    let registry = Registry::open(&fixture.state).unwrap();
    let registered = registry.register(&fixture.manifest, None, false).unwrap();
    let original = serde_json::to_value(&registered.manifest).unwrap();
    let connection = database(&registry);
    for after in [
        Value::Null,
        serde_json::json!([]),
        serde_json::json!(["test", "lint"]),
        serde_json::json!(["lint", "test", "build"]),
    ] {
        let mut snapshot = original.clone();
        snapshot["stages"][3]["after"] = after;
        connection
            .execute(
                "UPDATE registered_apps SET snapshot = ?1",
                [snapshot.to_string()],
            )
            .unwrap();
        assert_eq!(registry.get("demo").unwrap_err().code(), "storage");
    }
    let mut snapshot = original;
    snapshot["stages"][0]
        .as_object_mut()
        .unwrap()
        .remove("after");
    connection
        .execute(
            "UPDATE registered_apps SET snapshot = ?1",
            [snapshot.to_string()],
        )
        .unwrap();
    assert_eq!(registry.list().unwrap_err().code(), "storage");
}

#[test]
fn corrupt_cyclic_graph_yaml_is_rejected_even_with_matching_saved_hash_and_edges() {
    let fixture = Fixture::new(&MINIMAL.replace("[{id: build}]", "[{id: a}, {id: b}]"));
    let registry = Registry::open(&fixture.state).unwrap();
    let registered = registry.register(&fixture.manifest, None, false).unwrap();
    let mut snapshot = serde_json::to_value(&registered.manifest).unwrap();
    let yaml = registered
        .manifest
        .manifest_yaml
        .replace("{id: a}", "{id: a, after: [b]}");
    let hash = format!("{:x}", Sha256::digest(yaml.as_bytes()));
    snapshot["manifestYaml"] = Value::String(yaml);
    snapshot["manifestHash"] = Value::String(hash.clone());
    snapshot["stages"][0]["after"] = serde_json::json!(["b"]);
    database(&registry)
        .execute(
            "UPDATE registered_apps SET snapshot = ?1, manifest_hash = ?2",
            params![snapshot.to_string(), hash],
        )
        .unwrap();
    let error = registry.get("demo").unwrap_err();
    assert_eq!(error.code(), "storage");
    assert!(error.to_string().contains("cycle"), "{error}");
}
