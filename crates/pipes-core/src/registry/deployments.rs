use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, Row, TransactionBehavior, params, params_from_iter};

use super::Registry;
use crate::{
    ConsumerApproval, ConsumerDeployment, ConsumerSummary, Error, RecordConsumerDeployment, Result,
    validation,
};
use validation::{normalize_notes, normalize_ref};

const SELECT_DEPLOYMENT: &str = "SELECT id, app_id, app_name, stage_id, consumer_id, consumer_name, consumer_target, \
     consumer_approval, release_ref, notes, manifest_hash, manifest_path, source_path, \
     registration_updated_at, recorded_at FROM consumer_deployments";

impl Registry {
    /// Append a manually approved history marker using only the saved registration.
    ///
    /// Both review tokens must still match. No manifest/source files, tools,
    /// credentials, devices or networks are accessed, and no deployment is run.
    /// Repeating an approval intentionally appends another distinct marker.
    pub fn record_consumer_deployment(
        &self,
        request: &RecordConsumerDeployment,
    ) -> Result<ConsumerDeployment> {
        if !request.approved {
            return Err(Error::ApprovalRequired);
        }
        for (field, value) in [
            ("appId", &request.app_id),
            ("stageId", &request.stage_id),
            ("consumerId", &request.consumer_id),
        ] {
            validation::slug(value).map_err(|message| invalid_input(field, message))?;
        }
        validation::manifest_hash(&request.expected_manifest_hash)
            .map_err(|message| invalid_input("expectedManifestHash", message))?;
        validation::utc_timestamp(&request.expected_updated_at)
            .map_err(|message| invalid_input("expectedUpdatedAt", message))?;
        let release_ref = normalize_ref(&request.release_ref)
            .map_err(|message| invalid_input("releaseRef", message))?;
        let notes = normalize_notes(request.notes.as_deref())
            .map_err(|message| invalid_input("notes", message))?;

        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| self.storage("begin consumer deployment approval", error))?;
        self.require_version(&transaction)?;
        self.validate_schema(&transaction)?;
        let app = self.reviewed_app(
            &transaction,
            &request.app_id,
            &request.expected_manifest_hash,
            &request.expected_updated_at,
        )?;
        let consumer = app
            .manifest
            .stages
            .iter()
            .find(|stage| stage.id == request.stage_id)
            .and_then(|stage| {
                stage
                    .consumers
                    .iter()
                    .find(|consumer| consumer.id == request.consumer_id)
            })
            .ok_or_else(|| Error::ConsumerNotFound {
                app_id: request.app_id.clone(),
                stage_id: request.stage_id.clone(),
                consumer_id: request.consumer_id.clone(),
            })?;
        let approval = match consumer.approval {
            ConsumerApproval::Manual => "manual",
        };
        let recorded_at = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
        let inserted = transaction
            .execute(
                "INSERT INTO consumer_deployments \
                 (app_id, app_name, stage_id, consumer_id, consumer_name, consumer_target, \
                  consumer_approval, release_ref, notes, manifest_hash, manifest_path, source_path, \
                  registration_updated_at, recorded_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    app.manifest.app_id,
                    app.manifest.name,
                    request.stage_id,
                    consumer.id,
                    consumer.name,
                    consumer.target,
                    approval,
                    release_ref,
                    notes,
                    app.manifest.manifest_hash,
                    app.manifest.manifest_path,
                    app.manifest.source_path,
                    app.updated_at,
                    recorded_at,
                ],
            )
            .map_err(|error| self.storage("append consumer deployment history", error))?;
        let id = transaction.last_insert_rowid();
        if inserted != 1 || id <= 0 {
            return Err(self.storage(
                "append consumer deployment history",
                "expected one marker with a positive insertion ID",
            ));
        }
        let deployment = ConsumerDeployment {
            id: id.to_string(),
            app_id: app.manifest.app_id,
            app_name: app.manifest.name,
            stage_id: request.stage_id.clone(),
            consumer: consumer.clone(),
            release_ref,
            notes,
            manifest_hash: app.manifest.manifest_hash,
            manifest_path: app.manifest.manifest_path,
            source_path: app.manifest.source_path,
            registration_updated_at: app.updated_at,
            recorded_at,
        };
        transaction
            .commit()
            .map_err(|error| self.storage("commit consumer deployment history", error))?;
        Ok(deployment)
    }

    /// List immutable history by descending insertion ID, including unregistered
    /// apps. The optional app filter never consults the current registration.
    pub fn consumer_deployments(&self, app_id: Option<&str>) -> Result<Vec<ConsumerDeployment>> {
        if let Some(app_id) = app_id {
            validation::slug(app_id).map_err(|message| invalid_input("appId", message))?;
        }
        let connection = self.connection()?;
        self.load_consumer_deployments(&connection, app_id)
    }

    pub(super) fn load_consumer_deployments(
        &self,
        connection: &Connection,
        app_id: Option<&str>,
    ) -> Result<Vec<ConsumerDeployment>> {
        let filter = if app_id.is_some() {
            " WHERE app_id = ?1"
        } else {
            ""
        };
        let mut statement = connection
            .prepare(&format!("{SELECT_DEPLOYMENT}{filter} ORDER BY id DESC"))
            .map_err(|error| self.storage("list consumer deployments", error))?;
        let rows = statement
            .query_map(params_from_iter(app_id), StoredDeployment::read)
            .map_err(|error| self.storage("list consumer deployments", error))?;
        rows.map(|row| {
            row.map_err(|error| self.storage("read consumer deployment", error))?
                .decode(self)
        })
        .collect()
    }
}

fn invalid_input(field: &str, message: impl Into<String>) -> Error {
    Error::InvalidDeployment {
        field: field.to_owned(),
        message: message.into(),
    }
}

struct StoredDeployment {
    id: i64,
    app_id: String,
    app_name: String,
    stage_id: String,
    consumer_id: String,
    consumer_name: String,
    consumer_target: String,
    consumer_approval: String,
    release_ref: String,
    notes: Option<String>,
    manifest_hash: String,
    manifest_path: String,
    source_path: String,
    registration_updated_at: String,
    recorded_at: String,
}

impl StoredDeployment {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            app_id: row.get(1)?,
            app_name: row.get(2)?,
            stage_id: row.get(3)?,
            consumer_id: row.get(4)?,
            consumer_name: row.get(5)?,
            consumer_target: row.get(6)?,
            consumer_approval: row.get(7)?,
            release_ref: row.get(8)?,
            notes: row.get(9)?,
            manifest_hash: row.get(10)?,
            manifest_path: row.get(11)?,
            source_path: row.get(12)?,
            registration_updated_at: row.get(13)?,
            recorded_at: row.get(14)?,
        })
    }

    fn decode(self, registry: &Registry) -> Result<ConsumerDeployment> {
        let invalid = |message: String| {
            registry.storage(
                "decode consumer deployment",
                format!("marker '{}': {message}", self.id),
            )
        };
        if self.id <= 0 {
            return Err(invalid("insertion ID must be positive".to_owned()));
        }
        for (field, value) in [
            ("appId", &self.app_id),
            ("stageId", &self.stage_id),
            ("consumer.id", &self.consumer_id),
        ] {
            validation::slug(value).map_err(|message| invalid(format!("{field}: {message}")))?;
        }
        if self.app_name.trim().is_empty() || self.app_name.contains('\0') {
            return Err(invalid(
                "appName must be nonempty and contain no NUL characters".to_owned(),
            ));
        }
        for (field, value, limit) in [
            (
                "consumer.name",
                &self.consumer_name,
                validation::CONSUMER_NAME_LIMIT,
            ),
            (
                "consumer.target",
                &self.consumer_target,
                validation::CONSUMER_TARGET_LIMIT,
            ),
        ] {
            validation::display_text(value, limit)
                .map_err(|message| invalid(format!("{field}: {message}")))?;
        }
        if self.consumer_approval != "manual" {
            return Err(invalid("consumer.approval must be manual".to_owned()));
        }
        let release_ref = normalize_ref(&self.release_ref)
            .map_err(|message| invalid(format!("releaseRef: {message}")))?;
        let notes = normalize_notes(self.notes.as_deref())
            .map_err(|message| invalid(format!("notes: {message}")))?;
        if release_ref != self.release_ref || notes != self.notes {
            return Err(invalid(
                "releaseRef and notes must be stored in normalized form".to_owned(),
            ));
        }
        validation::manifest_hash(&self.manifest_hash)
            .map_err(|message| invalid(format!("manifestHash: {message}")))?;
        for (field, value) in [
            ("manifestPath", &self.manifest_path),
            ("sourcePath", &self.source_path),
        ] {
            validation::absolute_path(value)
                .map_err(|message| invalid(format!("{field}: {message}")))?;
        }
        for (field, value) in [
            ("registrationUpdatedAt", &self.registration_updated_at),
            ("recordedAt", &self.recorded_at),
        ] {
            validation::utc_timestamp(value)
                .map_err(|message| invalid(format!("{field}: {message}")))?;
        }
        Ok(ConsumerDeployment {
            id: self.id.to_string(),
            app_id: self.app_id,
            app_name: self.app_name,
            stage_id: self.stage_id,
            consumer: ConsumerSummary {
                id: self.consumer_id,
                name: self.consumer_name,
                target: self.consumer_target,
                approval: ConsumerApproval::Manual,
            },
            release_ref: self.release_ref,
            notes: self.notes,
            manifest_hash: self.manifest_hash,
            manifest_path: self.manifest_path,
            source_path: self.source_path,
            registration_updated_at: self.registration_updated_at,
            recorded_at: self.recorded_at,
        })
    }
}
