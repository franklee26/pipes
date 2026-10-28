use std::{
    env,
    ffi::OsString,
    fmt,
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use chrono::{SecondsFormat, Utc};
use directories::ProjectDirs;
use rusqlite::{
    Connection, ErrorCode, OpenFlags, OptionalExtension, Row, Transaction, TransactionBehavior,
    params,
};

use crate::{
    Error, ManifestPreview, RegisteredApp, Result,
    legacy::{ManifestPreviewV1, ManifestPreviewV2, ManifestPreviewV3, ManifestPreviewV4},
    manifest::{SnapshotVersion, preview_manifest_checked, validate_snapshot},
    validation,
};

mod deployments;
mod promotions;
pub(crate) mod runs;
mod schema;

const SCHEMA_VERSION: i64 = 6;
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(10);
const SELECT_APP: &str = "SELECT app_id, manifest_path, manifest_hash, snapshot, registered_at, \
                         updated_at FROM registered_apps";
const CREATE_SCHEMA: &str = "
    CREATE TABLE registered_apps (
        app_id TEXT NOT NULL PRIMARY KEY,
        manifest_path TEXT NOT NULL UNIQUE,
        manifest_hash TEXT NOT NULL,
        snapshot TEXT NOT NULL,
        registered_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
";

/// Resolve state storage without creating it.
///
/// A nonempty `PIPES_DATA_DIR` takes precedence over the platform-local
/// `ProjectDirs::from("dev", "pipes", "Pipes")` directory.
pub fn default_data_dir() -> Result<PathBuf> {
    resolve_data_dir(env::var_os("PIPES_DATA_DIR"), || {
        ProjectDirs::from("dev", "pipes", "Pipes")
            .map(|directories| directories.data_local_dir().to_owned())
    })
}

fn resolve_data_dir(
    override_dir: Option<OsString>,
    platform_default: impl FnOnce() -> Option<PathBuf>,
) -> Result<PathBuf> {
    match override_dir {
        Some(value) if value.is_empty() => Err(Error::InvalidDataDir {
            message: "PIPES_DATA_DIR is empty; set a directory or remove the override".to_owned(),
        }),
        Some(value) => Ok(PathBuf::from(value)),
        None => platform_default().ok_or_else(|| Error::InvalidDataDir {
            message:
                "cannot determine the platform data directory; set PIPES_DATA_DIR or --data-dir"
                    .to_owned(),
        }),
    }
}

/// A thread-safe registry handle containing paths, never a live SQLite connection.
///
/// Every operation opens its own connection. Mutations take an immediate
/// transaction so independent handles cannot silently overwrite each other.
#[derive(Clone, Debug)]
pub struct Registry {
    data_dir: PathBuf,
    database_path: PathBuf,
}

impl Registry {
    /// Open schema v6, atomically upgrading valid schema-v1 through v5 snapshots.
    /// Migration uses only stored YAML/JSON and preserves provenance and history.
    pub fn open(data_dir: impl Into<PathBuf>) -> Result<Self> {
        let data_dir = data_dir.into();
        if data_dir.as_os_str().is_empty() {
            return Err(Error::InvalidDataDir {
                message: "the data directory must not be empty".to_owned(),
            });
        }
        create_private_dir(&data_dir).map_err(|error| {
            Error::storage(&data_dir, format!("create data directory: {error}"))
        })?;
        let data_dir = fs::canonicalize(&data_dir).map_err(|error| {
            Error::storage(&data_dir, format!("resolve data directory: {error}"))
        })?;
        let registry = Self {
            database_path: data_dir.join("registry.sqlite3"),
            data_dir,
        };
        create_private_file(&registry.database_path)
            .map_err(|error| registry.storage("create registry file", error))?;
        let mut connection = registry.open_connection()?;
        registry.initialize_schema(&mut connection)?;
        registry.configure_wal(&connection)?;
        Ok(registry)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Return persisted snapshots sorted by app ID, without rereading manifests.
    pub fn list(&self) -> Result<Vec<RegisteredApp>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(&format!("{SELECT_APP} ORDER BY app_id COLLATE BINARY"))
            .map_err(|error| self.storage("list registered apps", error))?;
        let rows = statement
            .query_map([], StoredApp::read)
            .map_err(|error| self.storage("list registered apps", error))?;
        rows.map(|row| {
            self.decode(row.map_err(|error| self.storage("read registered app", error))?)
        })
        .collect()
    }

    pub fn get(&self, app_id: &str) -> Result<RegisteredApp> {
        let connection = self.connection()?;
        self.find(&connection, app_id)?
            .ok_or_else(|| Error::NotFound {
                app_id: app_id.to_owned(),
            })
    }

    /// Reread and validate a manifest before persisting its exact snapshot.
    ///
    /// Matching ID/manifest path/source path/hash is idempotent, including timestamps. Other changes
    /// to an existing app require `replace`; path collisions across app IDs never
    /// rename an existing registration. `expected_hash` protects a prior preview.
    pub fn register(
        &self,
        path: impl AsRef<Path>,
        expected_hash: Option<&str>,
        replace: bool,
    ) -> Result<RegisteredApp> {
        let manifest = preview_manifest_checked(path, expected_hash)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| self.storage("begin registration", error))?;
        self.require_version(&transaction)?;
        self.validate_schema(&transaction)?;
        let path_owner: Option<String> = transaction
            .query_row(
                "SELECT app_id FROM registered_apps WHERE manifest_path = ?1",
                [&manifest.manifest_path],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| self.storage("check manifest path ownership", error))?;
        if let Some(owner) = path_owner
            && owner != manifest.app_id
        {
            return Err(Error::AlreadyRegistered {
                app_id: owner,
                message: format!(
                    "manifest path '{}' belongs to another app ID; unregister that app explicitly before changing its identity, even with --replace",
                    manifest.manifest_path
                ),
            });
        }
        let existing = self.find(&transaction, &manifest.app_id)?;
        if let Some(existing) = &existing {
            if existing.manifest.manifest_path == manifest.manifest_path
                && existing.manifest.source_path == manifest.source_path
                && existing.manifest.manifest_hash == manifest.manifest_hash
            {
                transaction
                    .commit()
                    .map_err(|error| self.storage("finish idempotent registration", error))?;
                return Ok(existing.clone());
            }
            if !replace {
                return Err(Error::AlreadyRegistered {
                    app_id: manifest.app_id,
                    message: "its manifest/source path or contents changed; use --replace to update its saved snapshot"
                        .to_owned(),
                });
            }
        }
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
        let registered_at = existing
            .as_ref()
            .map_or_else(|| now.clone(), |app| app.registered_at.clone());
        let snapshot = serde_json::to_string(&manifest)
            .map_err(|error| self.storage("serialize manifest snapshot", error))?;
        if existing.is_some() {
            transaction
                .execute(
                    "UPDATE registered_apps SET manifest_path = ?2, manifest_hash = ?3, \
                     snapshot = ?4, updated_at = ?5 WHERE app_id = ?1",
                    params![
                        manifest.app_id,
                        manifest.manifest_path,
                        manifest.manifest_hash,
                        snapshot,
                        now
                    ],
                )
                .map_err(|error| self.storage("replace registered snapshot", error))?;
        } else {
            transaction
                .execute(
                    "INSERT INTO registered_apps \
                     (app_id, manifest_path, manifest_hash, snapshot, registered_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                    params![
                        manifest.app_id,
                        manifest.manifest_path,
                        manifest.manifest_hash,
                        snapshot,
                        now
                    ],
                )
                .map_err(|error| self.storage("insert registered snapshot", error))?;
        }
        transaction
            .commit()
            .map_err(|error| self.storage("commit registration", error))?;
        Ok(RegisteredApp {
            manifest,
            registered_at,
            updated_at: now,
        })
    }

    /// Remove only registration metadata, never evidence or approval history.
    /// No source or manifest files are touched.
    pub fn unregister(&self, app_id: &str) -> Result<()> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| self.storage("begin unregistration", error))?;
        self.require_version(&transaction)?;
        self.validate_schema(&transaction)?;
        self.find(&transaction, app_id)?
            .ok_or_else(|| Error::NotFound {
                app_id: app_id.to_owned(),
            })?;
        transaction
            .execute("DELETE FROM registered_apps WHERE app_id = ?1", [app_id])
            .map_err(|error| self.storage("delete registration metadata", error))?;
        transaction
            .commit()
            .map_err(|error| self.storage("commit unregistration", error))
    }

    fn storage(&self, action: &str, error: impl fmt::Display) -> Error {
        Error::storage(&self.database_path, format!("{action}: {error}"))
    }

    fn open_connection(&self) -> Result<Connection> {
        // No CREATE flag: deletion after Registry::open must not silently reset state.
        let connection = Connection::open_with_flags(
            &self.database_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|error| self.storage("open registry", error))?;
        connection
            .busy_timeout(SQLITE_BUSY_TIMEOUT)
            .map_err(|error| self.storage("set SQLite busy timeout", error))?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(|error| self.storage("set durable SQLite writes", error))?;
        Ok(connection)
    }

    fn connection(&self) -> Result<Connection> {
        let connection = self.open_connection()?;
        self.require_version(&connection)?;
        self.validate_schema(&connection)?;
        self.configure_wal(&connection)?;
        Ok(connection)
    }

    fn schema_version(&self, connection: &Connection) -> Result<i64> {
        connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|error| self.storage("read schema version", error))
    }

    fn require_version(&self, connection: &Connection) -> Result<()> {
        let found = self.schema_version(connection)?;
        if found != SCHEMA_VERSION {
            return Err(Error::UnsupportedSchema {
                path: self.database_path.clone(),
                found,
                supported: SCHEMA_VERSION,
            });
        }
        Ok(())
    }

    fn initialize_schema(&self, connection: &mut Connection) -> Result<()> {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| self.storage("begin schema initialization", error))?;
        let found = self.schema_version(&transaction)?;
        match found {
            0 => {
                let occupied: bool = transaction
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name NOT LIKE 'sqlite_%')",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| self.storage("check unversioned database", error))?;
                if occupied {
                    return Err(self.storage(
                        "initialize registry",
                        "unversioned database contains existing schema; refusing to reset it",
                    ));
                }
                transaction
                    .execute_batch(CREATE_SCHEMA)
                    .map_err(|error| self.storage("create registry schema", error))?;
                schema::create_deployments(self, &transaction)?;
                schema::create_promotions(self, &transaction)?;
                schema::create_runs(self, &transaction)?;
                transaction
                    .pragma_update(None, "user_version", SCHEMA_VERSION)
                    .map_err(|error| self.storage("save schema version", error))?;
            }
            1..=4 => {
                schema::validate_registered(self, &transaction)?;
                let version = match found {
                    1 => SnapshotVersion::LinearV1,
                    2 => SnapshotVersion::GraphV2,
                    3 => SnapshotVersion::PlansV3,
                    _ => SnapshotVersion::ConsumersV4,
                };
                if found < 4 {
                    schema::create_deployments(self, &transaction)?;
                } else {
                    schema::validate_deployments(self, &transaction)?;
                    self.load_consumer_deployments(&transaction, None)?;
                }
                schema::create_promotions(self, &transaction)?;
                self.migrate_legacy(&transaction, version)?;
                schema::create_runs(self, &transaction)?;
                transaction
                    .pragma_update(None, "user_version", SCHEMA_VERSION)
                    .map_err(|error| self.storage("save migrated schema version", error))?;
            }
            5 => {
                schema::validate_registered(self, &transaction)?;
                schema::validate_deployments(self, &transaction)?;
                schema::validate_promotions(self, &transaction)?;
                self.load_consumer_deployments(&transaction, None)?;
                self.load_harness_results(&transaction, None)?;
                self.load_promotion_approvals(&transaction, None)?;
                self.migrate_legacy(&transaction, SnapshotVersion::PromotionsV5)?;
                schema::create_runs(self, &transaction)?;
                transaction
                    .pragma_update(None, "user_version", SCHEMA_VERSION)
                    .map_err(|error| self.storage("save migrated schema version", error))?;
            }
            SCHEMA_VERSION => {}
            found => {
                return Err(Error::UnsupportedSchema {
                    path: self.database_path.clone(),
                    found,
                    supported: SCHEMA_VERSION,
                });
            }
        }
        self.validate_schema(&transaction)?;
        self.load_consumer_deployments(&transaction, None)?;
        self.load_harness_results(&transaction, None)?;
        self.load_promotion_approvals(&transaction, None)?;
        transaction
            .commit()
            .map_err(|error| self.storage("commit schema initialization", error))
    }

    fn migrate_legacy(
        &self,
        transaction: &Transaction<'_>,
        version: SnapshotVersion,
    ) -> Result<()> {
        let mut statement = transaction
            .prepare("SELECT app_id FROM registered_apps ORDER BY app_id COLLATE BINARY")
            .map_err(|error| self.storage("list legacy registrations", error))?;
        let app_ids = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| self.storage("read legacy app IDs", error))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| self.storage("read legacy app IDs", error))?;
        drop(statement);
        for app_id in app_ids {
            let record = transaction
                .query_row(
                    &format!("{SELECT_APP} WHERE app_id = ?1"),
                    [&app_id],
                    StoredApp::read,
                )
                .map_err(|error| self.storage("read legacy registration", error))?;
            let app = self.decode_versioned(record, version)?;
            let snapshot = serde_json::to_string(&app.manifest)
                .map_err(|error| self.storage("serialize migrated snapshot", error))?;
            let updated = transaction
                .execute(
                    "UPDATE registered_apps SET snapshot = ?1 WHERE app_id = ?2",
                    params![snapshot, app_id],
                )
                .map_err(|error| self.storage("upgrade legacy snapshot", error))?;
            if updated != 1 {
                return Err(self.storage(
                    "upgrade legacy snapshot",
                    format!(
                        "expected one saved registration for app '{app_id}', updated {updated}"
                    ),
                ));
            }
        }
        Ok(())
    }

    fn validate_schema(&self, connection: &Connection) -> Result<()> {
        schema::validate_registered(self, connection)?;
        schema::validate_deployments(self, connection)?;
        schema::validate_promotions(self, connection)?;
        schema::validate_runs(self, connection)
    }

    fn configure_wal(&self, connection: &Connection) -> Result<()> {
        let mode: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .map_err(|error| self.storage("read journal mode", error))?;
        if !mode.eq_ignore_ascii_case("wal") {
            let started = Instant::now();
            let mode: String = loop {
                match connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0)) {
                    Ok(mode) => break mode,
                    Err(error)
                        if matches!(
                            error.sqlite_error_code(),
                            Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
                        ) && started.elapsed() < SQLITE_BUSY_TIMEOUT =>
                    {
                        // Journal-mode lock upgrades can bypass busy_timeout while
                        // another handle initializes schema. Release and retry.
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => return Err(self.storage("enable WAL journal mode", error)),
                }
            };
            if !mode.eq_ignore_ascii_case("wal") {
                return Err(
                    self.storage("enable WAL journal mode", format!("SQLite returned {mode}"))
                );
            }
        }
        Ok(())
    }

    fn find(&self, connection: &Connection, app_id: &str) -> Result<Option<RegisteredApp>> {
        connection
            .query_row(
                &format!("{SELECT_APP} WHERE app_id = ?1"),
                [app_id],
                StoredApp::read,
            )
            .optional()
            .map_err(|error| self.storage("read registered app", error))?
            .map(|record| self.decode(record))
            .transpose()
    }

    fn decode(&self, record: StoredApp) -> Result<RegisteredApp> {
        self.decode_versioned(record, SnapshotVersion::RunsV6)
    }

    fn reviewed_app(
        &self,
        connection: &Connection,
        app_id: &str,
        expected_manifest_hash: &str,
        expected_updated_at: &str,
    ) -> Result<RegisteredApp> {
        let app = self
            .find(connection, app_id)?
            .ok_or_else(|| Error::NotFound {
                app_id: app_id.to_owned(),
            })?;
        if app.manifest.manifest_hash != expected_manifest_hash
            || app.updated_at != expected_updated_at
        {
            return Err(Error::RegistrationChanged {
                app_id: app_id.to_owned(),
            });
        }
        Ok(app)
    }

    fn decode_versioned(
        &self,
        record: StoredApp,
        version: SnapshotVersion,
    ) -> Result<RegisteredApp> {
        let invalid = |message: String| {
            self.storage(
                "decode registered snapshot",
                format!("app '{}': {message}", record.app_id),
            )
        };
        let manifest: ManifestPreview = match version {
            SnapshotVersion::LinearV1 => {
                serde_json::from_str::<ManifestPreviewV1>(&record.snapshot)
                    .map(ManifestPreviewV1::into_current)
            }
            SnapshotVersion::GraphV2 => serde_json::from_str::<ManifestPreviewV2>(&record.snapshot)
                .map(ManifestPreviewV2::into_current),
            SnapshotVersion::PlansV3 => serde_json::from_str::<ManifestPreviewV3>(&record.snapshot)
                .map(ManifestPreviewV3::into_current),
            SnapshotVersion::ConsumersV4 => {
                serde_json::from_str::<ManifestPreviewV4>(&record.snapshot)
                    .map(ManifestPreviewV4::into_current)
            }
            SnapshotVersion::PromotionsV5 | SnapshotVersion::RunsV6 => {
                serde_json::from_str(&record.snapshot)
            }
        }
        .map_err(|error| invalid(format!("invalid snapshot JSON: {error}")))?;
        if record.app_id != manifest.app_id
            || record.manifest_path != manifest.manifest_path
            || record.manifest_hash != manifest.manifest_hash
        {
            return Err(invalid(
                "indexed app ID, path, or hash does not match its snapshot".to_owned(),
            ));
        }
        validate_snapshot(&manifest, version)
            .map_err(|error| invalid(format!("corrupt manifest snapshot: {error}")))?;
        for (field, value) in [
            ("registered_at", &record.registered_at),
            ("updated_at", &record.updated_at),
        ] {
            validation::utc_timestamp(value)
                .map_err(|message| invalid(format!("{field}: {message}")))?;
        }
        Ok(RegisteredApp {
            manifest,
            registered_at: record.registered_at,
            updated_at: record.updated_at,
        })
    }
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn create_private_file(path: &Path) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(path)?.file_type().is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "registry.sqlite3 must be a regular file, not a symlink or directory",
                ));
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

struct StoredApp {
    app_id: String,
    manifest_path: String,
    manifest_hash: String,
    snapshot: String,
    registered_at: String,
    updated_at: String,
}

impl StoredApp {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            app_id: row.get(0)?,
            manifest_path: row.get(1)?,
            manifest_hash: row.get(2)?,
            snapshot: row.get(3)?,
            registered_at: row.get(4)?,
            updated_at: row.get(5)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Registry>();
    }

    #[test]
    fn wal_transition_retries_schema_initializer_contention() {
        let directory = tempfile::tempdir().unwrap();
        let registry = Registry::open(directory.path()).unwrap();
        let mut blocker = registry.open_connection().unwrap();
        let mode: String = blocker
            .query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "delete");
        let transaction = blocker
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker_registry = registry.clone();
        let worker = std::thread::spawn(move || {
            let connection = worker_registry.open_connection().unwrap();
            sender
                .send(worker_registry.configure_wal(&connection))
                .unwrap();
        });
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        transaction.commit().unwrap();
        receiver.recv_timeout(SQLITE_BUSY_TIMEOUT).unwrap().unwrap();
        worker.join().unwrap();
        drop(blocker);
        let mode: String = registry
            .open_connection()
            .unwrap()
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn state_directory_resolution_does_not_need_global_environment_mutation() {
        let default = PathBuf::from("platform-local");
        assert_eq!(
            resolve_data_dir(None, || Some(default.clone())).unwrap(),
            default
        );
        assert_eq!(
            resolve_data_dir(Some(OsString::from("state with spaces")), || None).unwrap(),
            PathBuf::from("state with spaces")
        );
        assert_eq!(
            resolve_data_dir(Some(OsString::new()), || Some(default))
                .unwrap_err()
                .code(),
            "invalid_data_dir"
        );
        assert_eq!(
            resolve_data_dir(None, || None).unwrap_err().code(),
            "invalid_data_dir"
        );
    }
}
