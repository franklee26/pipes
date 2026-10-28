use rusqlite::{Connection, OptionalExtension, Transaction};

use super::{CREATE_SCHEMA, Registry};
use crate::Result;

const CREATE_DEPLOYMENTS: &str = "
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
";

const CREATE_DEPLOYMENT_INDEX: &str =
    "CREATE INDEX consumer_deployments_app_id ON consumer_deployments(app_id);";

const CREATE_HARNESS_RESULTS: &str = "
    CREATE TABLE harness_results (
        id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
        app_id TEXT NOT NULL,
        app_name TEXT NOT NULL,
        stage_id TEXT NOT NULL,
        check_id TEXT NOT NULL,
        release_ref TEXT NOT NULL,
        total INTEGER NOT NULL,
        passed INTEGER NOT NULL,
        notes TEXT,
        manifest_hash TEXT NOT NULL,
        manifest_path TEXT NOT NULL,
        source_path TEXT NOT NULL,
        registration_updated_at TEXT NOT NULL,
        recorded_at TEXT NOT NULL
    );
";

const CREATE_HARNESS_INDEX: &str =
    "CREATE INDEX harness_results_app_id ON harness_results(app_id);";

const CREATE_PROMOTION_APPROVALS: &str = "
    CREATE TABLE promotion_approvals (
        id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
        app_id TEXT NOT NULL,
        assessment TEXT NOT NULL,
        skipped_optional TEXT NOT NULL,
        notes TEXT,
        recorded_at TEXT NOT NULL
    );
";

const CREATE_APPROVAL_INDEX: &str =
    "CREATE INDEX promotion_approvals_app_id ON promotion_approvals(app_id);";

const CREATE_RUNS: &str = "
    CREATE TABLE pipeline_runs (
        id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
        app_id TEXT NOT NULL,
        source_path TEXT NOT NULL,
        snapshot TEXT NOT NULL,
        active INTEGER NOT NULL,
        cancel_requested INTEGER NOT NULL,
        stage_approvals TEXT NOT NULL
    );
";

const CREATE_RUN_INDEX: &str = "CREATE INDEX pipeline_runs_app_id ON pipeline_runs(app_id);";

const CREATE_RUN_LOGS: &str = "
    CREATE TABLE pipeline_run_logs (
        id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
        run_id INTEGER NOT NULL,
        recorded_at TEXT NOT NULL,
        stage_id TEXT,
        command_id TEXT,
        stream TEXT NOT NULL,
        text TEXT NOT NULL
    );
";

const CREATE_RUN_LOG_INDEX: &str =
    "CREATE INDEX pipeline_run_logs_run_id ON pipeline_run_logs(run_id);";

pub(super) fn create_runs(registry: &Registry, transaction: &Transaction<'_>) -> Result<()> {
    for sql in [
        CREATE_RUNS,
        CREATE_RUN_INDEX,
        CREATE_RUN_LOGS,
        CREATE_RUN_LOG_INDEX,
    ] {
        transaction
            .execute_batch(sql)
            .map_err(|error| registry.storage("create pipeline run history", error))?;
    }
    Ok(())
}

pub(super) fn validate_runs(registry: &Registry, connection: &Connection) -> Result<()> {
    validate_history(
        registry,
        connection,
        "pipeline_runs",
        CREATE_RUNS,
        "pipeline_runs_app_id",
        CREATE_RUN_INDEX,
    )?;
    validate_history(
        registry,
        connection,
        "pipeline_run_logs",
        CREATE_RUN_LOGS,
        "pipeline_run_logs_run_id",
        CREATE_RUN_LOG_INDEX,
    )
}

pub(super) fn create_deployments(registry: &Registry, transaction: &Transaction<'_>) -> Result<()> {
    transaction
        .execute_batch(CREATE_DEPLOYMENTS)
        .and_then(|()| transaction.execute_batch(CREATE_DEPLOYMENT_INDEX))
        .map_err(|error| registry.storage("create consumer deployment history", error))
}

pub(super) fn create_promotions(registry: &Registry, transaction: &Transaction<'_>) -> Result<()> {
    for sql in [
        CREATE_HARNESS_RESULTS,
        CREATE_HARNESS_INDEX,
        CREATE_PROMOTION_APPROVALS,
        CREATE_APPROVAL_INDEX,
    ] {
        transaction.execute_batch(sql).map_err(|error| {
            registry.storage("create promotion evidence and approval history", error)
        })?;
    }
    Ok(())
}

pub(super) fn validate_registered(registry: &Registry, connection: &Connection) -> Result<()> {
    validate_unique_indexes(
        registry,
        connection,
        "registered_apps",
        &["app_id", "manifest_path"],
    )?;
    validate_definition(
        registry,
        connection,
        "table",
        "registered_apps",
        CREATE_SCHEMA,
    )?;
    reject_triggers(registry, connection, "registered_apps")
}

pub(super) fn validate_deployments(registry: &Registry, connection: &Connection) -> Result<()> {
    validate_history(
        registry,
        connection,
        "consumer_deployments",
        CREATE_DEPLOYMENTS,
        "consumer_deployments_app_id",
        CREATE_DEPLOYMENT_INDEX,
    )
}

pub(super) fn validate_promotions(registry: &Registry, connection: &Connection) -> Result<()> {
    validate_history(
        registry,
        connection,
        "harness_results",
        CREATE_HARNESS_RESULTS,
        "harness_results_app_id",
        CREATE_HARNESS_INDEX,
    )?;
    validate_history(
        registry,
        connection,
        "promotion_approvals",
        CREATE_PROMOTION_APPROVALS,
        "promotion_approvals_app_id",
        CREATE_APPROVAL_INDEX,
    )
}

fn validate_history(
    registry: &Registry,
    connection: &Connection,
    table: &str,
    definition: &str,
    index: &str,
    index_definition: &str,
) -> Result<()> {
    validate_definition(registry, connection, "table", table, definition)?;
    validate_unique_indexes(registry, connection, table, &[])?;
    validate_definition(registry, connection, "index", index, index_definition)?;
    reject_triggers(registry, connection, table)?;
    validate_sequence(registry, connection, table)
}

fn validate_sequence(registry: &Registry, connection: &Connection, table: &str) -> Result<()> {
    let (largest, count, sequence): (Option<i64>, i64, Option<i64>) = connection
        .query_row(
            &format!(
                "SELECT (SELECT MAX(id) FROM {table}), \
                 (SELECT count(*) FROM sqlite_sequence WHERE name = ?1), \
                 (SELECT seq FROM sqlite_sequence WHERE name = ?1 LIMIT 1)"
            ),
            [table],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| registry.storage("inspect history insertion sequence", error))?;
    let valid = match (count, sequence) {
        (0, None) => largest.is_none(),
        (1, Some(id)) => id >= 0 && largest.is_none_or(|largest| id >= largest),
        _ => false,
    };
    if !valid {
        return Err(registry.storage(
            "validate history insertion sequence",
            format!("{table} has a missing or corrupt AUTOINCREMENT sequence; refusing to reuse history IDs"),
        ));
    }
    Ok(())
}

fn validate_definition(
    registry: &Registry,
    connection: &Connection,
    kind: &str,
    name: &str,
    expected: &str,
) -> Result<()> {
    let definition: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = ?1 AND name = ?2",
            [kind, name],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| registry.storage("inspect schema definition", error))?;
    // Frozen declarations also check AUTOINCREMENT, nullability, defaults, foreign
    // keys, generated columns and conflict policies that table_info cannot prove.
    if !definition.is_some_and(|sql| normalize_sql(&sql) == normalize_sql(expected)) {
        return Err(registry.storage(
            "validate schema",
            format!("{name} is missing or incompatible; the database was not reset"),
        ));
    }
    Ok(())
}

fn normalize_sql(sql: &str) -> String {
    let mut normalized = String::new();
    let mut word = false;
    let mut separated = false;
    for character in sql.trim().trim_end_matches(';').chars() {
        if character.is_ascii_whitespace() {
            separated = true;
            continue;
        }
        let next_word = character.is_ascii_alphanumeric() || character == '_';
        if separated && word && next_word {
            normalized.push(' ');
        }
        normalized.push(character.to_ascii_lowercase());
        word = next_word;
        separated = false;
    }
    normalized
}

fn reject_triggers(registry: &Registry, connection: &Connection, table: &str) -> Result<()> {
    let trigger: Option<String> = connection
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = ?1 LIMIT 1",
            [table],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| registry.storage("inspect registry triggers", error))?;
    if let Some(trigger) = trigger {
        return Err(registry.storage(
            "validate schema",
            format!("unexpected trigger '{trigger}' on {table}; refusing unsafe writes"),
        ));
    }
    Ok(())
}

fn validate_unique_indexes(
    registry: &Registry,
    connection: &Connection,
    table: &str,
    expected: &[&str],
) -> Result<()> {
    let mut statement = connection
        .prepare("SELECT name, partial FROM pragma_index_list(?1) WHERE \"unique\" = 1")
        .map_err(|error| registry.storage("inspect uniqueness constraints", error))?;
    let indexes = statement
        .query_map([table], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
        })
        .map_err(|error| registry.storage("read uniqueness constraints", error))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| registry.storage("read uniqueness constraints", error))?;
    let invalid = || {
        registry.storage(
            "validate schema",
            format!(
                "{table} uniqueness constraints are missing or incompatible; refusing unsafe writes"
            ),
        )
    };
    if indexes.len() != expected.len() {
        return Err(invalid());
    }
    let mut found = Vec::new();
    for (index, partial) in indexes {
        let mut statement = connection
            .prepare(
                "SELECT name, coll, desc FROM pragma_index_xinfo(?1) WHERE key = 1 ORDER BY seqno",
            )
            .map_err(|error| registry.storage("inspect unique index", error))?;
        let columns = statement
            .query_map([index], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            })
            .map_err(|error| registry.storage("read unique index", error))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| registry.storage("read unique index", error))?;
        let [(column, collation, descending)] = columns.as_slice() else {
            return Err(invalid());
        };
        if partial
            || collation != "BINARY"
            || *descending
            || !expected.contains(&column.as_str())
            || found.contains(column)
        {
            return Err(invalid());
        }
        found.push(column.clone());
    }
    Ok(())
}
