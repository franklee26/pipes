#![allow(dead_code)]

use pipes_core::{
    ApprovePromotion, PromotionAssessment, PromotionContext, RecordConsumerDeployment,
    RecordHarnessResult, RegisteredApp, Registry,
};
use rusqlite::Connection;

use crate::support::Fixture;

pub const HARNESS: &str =
    "{id: smoke-tests, kind: harness, check: smoke, expected_tests: 50, minimum_passed: 45}";
pub const REQUIRED: &str =
    "{id: lab-delivery, kind: deployment, consumer: lab-device, requirement: required}";
pub const OPTIONAL: &str =
    "{id: personal-delivery, kind: deployment, consumer: personal-device, requirement: optional}";
pub const SPARE: &str =
    "{id: spare-delivery, kind: deployment, consumer: spare-device, requirement: optional}";

pub const BASE: &str = r#"api: pipes/v1
app: demo
name: Demo Application
source: {path: source}
version: {from: file:VERSION}
execution: {max_parallel: 4}
prepare:
  - id: setup
    exec: [unavailable-prepare]
stages:
  - id: draft
    checks: [{id: elsewhere, exec: [unavailable-check]}]
    consumers: [{id: foreign, target: elsewhere}]
  - id: tested
    checks:
      - id: batch
        max_parallel: 2
        parallel:
          - id: ordered
            sequence:
              - id: smoke
                exec: [unavailable-harness]
      - id: extra
        exec: [unavailable-extra]
    consumers:
      - {id: lab-device, name: Lab Device, target: "lab / applications"}
      - {id: personal-device, target: "personal / applications"}
      - {id: spare-device, target: "spare / applications"}
    require: [checks.passed, previous.passed, prepare.setup.passed, approval.local]
  - id: released
    require: [previous.passed]
"#;

pub fn yaml_with_promotion(promotion: &str) -> String {
    BASE.replace(
        "  - id: tested\n",
        &format!("  - id: tested\n    promotion: {promotion}\n"),
    )
}

pub fn yaml(criteria: &str) -> String {
    yaml_with_promotion(&format!("{{criteria: [{criteria}]}}"))
}

pub fn setup(criteria: &str) -> (Fixture, Registry, RegisteredApp) {
    let fixture = Fixture::new(&yaml(criteria));
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    (fixture, registry, app)
}

pub fn context(app: &RegisteredApp) -> PromotionContext {
    PromotionContext {
        app_id: app.manifest.app_id.clone(),
        stage_id: "tested".to_owned(),
        expected_manifest_hash: app.manifest.manifest_hash.clone(),
        expected_updated_at: app.updated_at.clone(),
        release_ref: "1.2.3+candidate".to_owned(),
    }
}

pub fn report(context: &PromotionContext, total: u32, passed: u32) -> RecordHarnessResult {
    RecordHarnessResult {
        app_id: context.app_id.clone(),
        stage_id: context.stage_id.clone(),
        expected_manifest_hash: context.expected_manifest_hash.clone(),
        expected_updated_at: context.expected_updated_at.clone(),
        release_ref: context.release_ref.clone(),
        check_id: "smoke".to_owned(),
        total,
        passed,
        notes: None,
    }
}

pub fn deployment(context: &PromotionContext, consumer_id: &str) -> RecordConsumerDeployment {
    RecordConsumerDeployment {
        app_id: context.app_id.clone(),
        stage_id: context.stage_id.clone(),
        consumer_id: consumer_id.to_owned(),
        expected_manifest_hash: context.expected_manifest_hash.clone(),
        expected_updated_at: context.expected_updated_at.clone(),
        release_ref: context.release_ref.clone(),
        notes: None,
        approved: true,
    }
}

pub fn approval(assessment: &PromotionAssessment) -> ApprovePromotion {
    ApprovePromotion {
        app_id: assessment.app_id.clone(),
        stage_id: assessment.stage_id.clone(),
        expected_manifest_hash: assessment.manifest_hash.clone(),
        expected_updated_at: assessment.registration_updated_at.clone(),
        release_ref: assessment.release_ref.clone(),
        expected_assessment_hash: assessment.assessment_hash.clone(),
        approved: true,
        acknowledge_optional: false,
        notes: None,
    }
}

pub fn database(registry: &Registry) -> Connection {
    Connection::open(registry.data_dir().join("registry.sqlite3")).unwrap()
}
