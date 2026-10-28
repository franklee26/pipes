mod promotion;
#[allow(dead_code)]
mod support;

use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

use chrono::{DateTime, Utc};
use pipes_core::{
    ApprovePromotion, CriterionStatus, HarnessCounts, HarnessResult, PromotionApproval,
    PromotionAssessment, PromotionContext, PromotionStatus, RecordHarnessResult, Registry,
};
use promotion::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::Fixture;

#[test]
fn exact_total_and_minimum_passes_use_latest_report_including_zero_and_mismatched_totals() {
    let (_fixture, registry, app) = setup(HARNESS);
    let context = context(&app);
    let missing = registry.assess_promotion(&context).unwrap();
    assert_eq!(missing.status, PromotionStatus::Blocked);
    assert_eq!(missing.criteria[0].observed, None);
    assert_eq!(missing.criteria[0].evidence_id, None);
    assert_eq!(missing.stage_requirements, app.manifest.stages[1].requires);
    for (total, passed, expected) in [
        (50, 44, PromotionStatus::Blocked),
        (50, 45, PromotionStatus::Ready),
        (49, 45, PromotionStatus::Blocked),
        (51, 50, PromotionStatus::Blocked),
        (0, 0, PromotionStatus::Blocked),
        (50, 50, PromotionStatus::Ready),
        (50, 1, PromotionStatus::Blocked),
    ] {
        let report = registry
            .record_harness_result(&report(&context, total, passed))
            .unwrap();
        let assessment = registry.assess_promotion(&context).unwrap();
        assert_eq!(assessment.status, expected, "{passed}/{total}");
        assert_eq!(
            assessment.criteria[0].evidence_id.as_deref(),
            Some(report.id.as_str())
        );
        assert_eq!(
            assessment.criteria[0].observed,
            Some(HarnessCounts { total, passed })
        );
        assert!(
            assessment.criteria[0]
                .message
                .contains("exactly 50 tests and at least 45 passed")
        );
        assert!(assessment.criteria[0].consumer.is_none());
    }
    assert_eq!(registry.harness_results(None).unwrap().len(), 7);
    assert_eq!(registry.get("demo").unwrap(), app);
}

#[test]
fn all_pass_and_maximum_u32_policies_keep_integer_semantics() {
    for n in [1, 50, u32::MAX] {
        let criterion = HARNESS
            .replace("expected_tests: 50", &format!("expected_tests: {n}"))
            .replace("minimum_passed: 45", &format!("minimum_passed: {n}"));
        let (_fixture, registry, app) = setup(&criterion);
        let context = context(&app);
        registry
            .record_harness_result(&report(&context, n, n - 1))
            .unwrap();
        assert_eq!(
            registry.assess_promotion(&context).unwrap().status,
            PromotionStatus::Blocked
        );
        let result = registry
            .record_harness_result(&report(&context, n, n))
            .unwrap();
        assert_eq!(result.total, n);
        assert_eq!(
            registry.assess_promotion(&context).unwrap().status,
            PromotionStatus::Ready
        );
    }
}

#[test]
fn reporting_normalizes_unicode_text_and_exposes_flat_number_based_dtos() {
    let (_fixture, registry, app) = setup(HARNESS);
    let mut context = context(&app);
    context.release_ref = format!("  {}  ", "\u{e9}".repeat(256));
    let mut request = report(&context, 50, 45);
    request.notes = Some(format!("  {}\n\t{}  ", "\u{e9}".repeat(4093), "\u{1f4bb}"));
    let before = Utc::now();
    let result = registry.record_harness_result(&request).unwrap();
    let after = Utc::now();
    assert_eq!(result.release_ref.chars().count(), 256);
    assert_eq!(result.notes.as_deref().unwrap().chars().count(), 4096);
    let timestamp = DateTime::parse_from_rfc3339(&result.recorded_at).unwrap();
    assert!(timestamp >= before && timestamp <= after);
    assert_eq!(timestamp.offset().local_minus_utc(), 0);
    let encoded = serde_json::to_value(&result).unwrap();
    assert_eq!(encoded["total"], json!(50));
    assert_eq!(encoded["passed"], json!(45));
    assert_eq!(encoded["id"], "1");
    assert_eq!(encoded["manifestPath"], app.manifest.manifest_path);
    assert_eq!(encoded["registrationUpdatedAt"], app.updated_at);
    assert_eq!(
        serde_json::from_value::<HarnessResult>(encoded).unwrap(),
        result
    );
    let assessment = registry.assess_promotion(&context).unwrap();
    let json = serde_json::to_value(&assessment).unwrap();
    assert!(json.get("recordedAt").is_none());
    assert_eq!(json["criteria"][0]["status"], "satisfied");
    assert_eq!(
        json["criteria"][0]["observed"],
        json!({"total": 50, "passed": 45})
    );
    assert_eq!(
        serde_json::from_value::<PromotionAssessment>(json.clone()).unwrap(),
        assessment
    );
    let mut contents = json;
    contents.as_object_mut().unwrap().remove("assessmentHash");
    assert_eq!(
        assessment.assessment_hash,
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&contents).unwrap())
        )
    );
    assert_eq!(registry.get("demo").unwrap(), app);
    for notes in [None, Some(" \n\t ".to_owned())] {
        request.notes = notes;
        assert!(
            registry
                .record_harness_result(&request)
                .unwrap()
                .notes
                .is_none()
        );
    }
}

#[test]
fn requests_reject_unknown_fields_wrong_types_and_missing_required_acknowledgements() {
    let (_fixture, registry, app) = setup(HARNESS);
    let context = context(&app);
    let request = serde_json::to_value(report(&context, 50, 45)).unwrap();
    assert!(request.get("context").is_none());
    assert_eq!(request["expectedManifestHash"], app.manifest.manifest_hash);
    for (field, value) in [
        ("approved", json!(true)),
        ("assessment", json!({})),
        ("check_id", json!("smoke")),
        ("total", json!(-1)),
        ("total", json!(4294967296_u64)),
        ("passed", json!(45.5)),
        ("total", json!("50")),
        ("passed", Value::Null),
    ] {
        let mut changed = request.clone();
        changed[field] = value;
        assert!(
            serde_json::from_value::<RecordHarnessResult>(changed).is_err(),
            "{field}"
        );
    }
    registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    let assessment = registry.assess_promotion(&context).unwrap();
    let request = serde_json::to_value(approval(&assessment)).unwrap();
    for field in ["approved", "acknowledgeOptional", "expectedAssessmentHash"] {
        let mut changed = request.clone();
        changed.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<ApprovePromotion>(changed).is_err());
    }
    for field in ["assessment", "target", "skippedOptional"] {
        let mut changed = request.clone();
        changed[field] = json!({});
        assert!(serde_json::from_value::<ApprovePromotion>(changed).is_err());
    }
    let mut changed = serde_json::to_value(&context).unwrap();
    changed["unknown"] = json!(true);
    assert!(serde_json::from_value::<PromotionContext>(changed).is_err());
}

#[test]
fn invalid_reports_and_contexts_fail_before_writes_with_stable_codes() {
    let (_fixture, registry, app) = setup(HARNESS);
    let context = context(&app);
    let original = report(&context, 50, 45);
    let mut invalid = Vec::new();
    for value in ["", " ", "release\n", "\tref", "a\0b", "a\u{7f}b"] {
        let mut request = original.clone();
        request.release_ref = value.to_owned();
        invalid.push(request);
    }
    let mut request = original.clone();
    request.release_ref = "\u{1f4bb}".repeat(257);
    invalid.push(request);
    for value in ["\u{e9}".repeat(4097), "a\rb".to_owned(), "a\0b".to_owned()] {
        let mut request = original.clone();
        request.notes = Some(value);
        invalid.push(request);
    }
    for field in [
        "appId",
        "stageId",
        "checkId",
        "expectedManifestHash",
        "expectedUpdatedAt",
    ] {
        let mut request = serde_json::to_value(&original).unwrap();
        request[field] = json!("invalid value");
        invalid.push(serde_json::from_value(request).unwrap());
    }
    let mut request = original.clone();
    request.passed = 51;
    invalid.push(request);
    for request in invalid {
        assert_eq!(
            registry.record_harness_result(&request).unwrap_err().code(),
            "invalid_harness_result"
        );
    }
    for check in ["batch", "ordered", "setup", "elsewhere", "missing"] {
        let mut request = original.clone();
        request.check_id = check.to_owned();
        assert_eq!(
            registry.record_harness_result(&request).unwrap_err().code(),
            "check_not_found"
        );
    }
    let mut missing = context.clone();
    missing.stage_id = "missing".to_owned();
    assert_eq!(
        registry
            .record_harness_result(&report(&missing, 1, 1))
            .unwrap_err()
            .code(),
        "stage_not_found"
    );
    assert_eq!(
        registry.assess_promotion(&missing).unwrap_err().code(),
        "stage_not_found"
    );
    missing.app_id = "absent".to_owned();
    assert_eq!(
        registry.assess_promotion(&missing).unwrap_err().code(),
        "not_found"
    );
    assert_eq!(
        registry
            .record_harness_result(&report(&missing, 1, 1))
            .unwrap_err()
            .code(),
        "not_found"
    );
    let mut invalid_context = context.clone();
    invalid_context.release_ref.clear();
    assert_eq!(
        registry
            .assess_promotion(&invalid_context)
            .unwrap_err()
            .code(),
        "invalid_promotion"
    );
    for (field, value) in [
        ("expectedManifestHash", "a".repeat(64)),
        ("expectedUpdatedAt", "2000-01-01T00:00:00Z".to_owned()),
    ] {
        let mut stale = serde_json::to_value(&context).unwrap();
        stale[field] = json!(value);
        let stale: PromotionContext = serde_json::from_value(stale).unwrap();
        assert_eq!(
            registry.assess_promotion(&stale).unwrap_err().code(),
            "registration_changed"
        );
        assert_eq!(
            registry
                .record_harness_result(&report(&stale, 1, 1))
                .unwrap_err()
                .code(),
            "registration_changed"
        );
    }
    assert!(registry.harness_results(None).unwrap().is_empty());
}

#[test]
fn required_targets_block_optional_targets_warn_and_approval_freezes_acknowledged_skips() {
    let (fixture, registry, app) = setup(&format!("{HARNESS}, {REQUIRED}, {OPTIONAL}, {SPARE}"));
    let context = context(&app);
    registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    let blocked = registry.assess_promotion(&context).unwrap();
    assert_eq!(blocked.status, PromotionStatus::Blocked);
    assert_eq!(blocked.criteria[1].status, CriterionStatus::Blocked);
    assert_eq!(blocked.criteria[2].status, CriterionStatus::Warning);
    assert_eq!(
        blocked.criteria[1].consumer.as_ref(),
        Some(&app.manifest.stages[1].consumers[0])
    );
    assert_eq!(
        registry
            .approve_promotion(&approval(&blocked))
            .unwrap_err()
            .code(),
        "promotion_blocked"
    );
    let lab = registry
        .record_consumer_deployment(&deployment(&context, "lab-device"))
        .unwrap();
    let warnings = registry.assess_promotion(&context).unwrap();
    assert_eq!(warnings.status, PromotionStatus::ReadyWithWarnings);
    assert_eq!(warnings.criteria[1].evidence_id, Some(lab.id));
    let mut request = approval(&warnings);
    request.approved = false;
    assert_eq!(
        registry.approve_promotion(&request).unwrap_err().code(),
        "approval_required"
    );
    request.approved = true;
    assert_eq!(
        registry.approve_promotion(&request).unwrap_err().code(),
        "optional_steps_unacknowledged"
    );
    request.acknowledge_optional = true;
    request.notes = Some("  Reviewed criteria only\n\tSkipped personal devices  ".to_owned());
    let before = Utc::now();
    let saved = registry.approve_promotion(&request).unwrap();
    let after = Utc::now();
    assert_eq!(saved.assessment, warnings);
    assert_eq!(
        saved.skipped_optional,
        ["personal-delivery", "spare-delivery"]
    );
    assert_eq!(
        saved.notes.as_deref(),
        Some("Reviewed criteria only\n\tSkipped personal devices")
    );
    let recorded = DateTime::parse_from_rfc3339(&saved.recorded_at).unwrap();
    assert!(recorded >= before && recorded <= after);
    assert_eq!(recorded.offset().local_minus_utc(), 0);
    assert_eq!(
        serde_json::from_value::<PromotionApproval>(serde_json::to_value(&saved).unwrap()).unwrap(),
        saved
    );
    for id in ["personal-device", "spare-device"] {
        registry
            .record_consumer_deployment(&deployment(&context, id))
            .unwrap();
    }
    let ready = registry.assess_promotion(&context).unwrap();
    assert_eq!(ready.status, PromotionStatus::Ready);
    assert_eq!(
        registry.approve_promotion(&request).unwrap_err().code(),
        "assessment_changed"
    );
    let complete = registry.approve_promotion(&approval(&ready)).unwrap();
    assert!(complete.skipped_optional.is_empty());
    assert_eq!(
        registry.promotion_approvals(None).unwrap(),
        [complete.clone(), saved.clone()]
    );
    assert_eq!(registry.get("demo").unwrap(), app);
    registry.unregister("demo").unwrap();
    assert_eq!(
        Registry::open(&fixture.state)
            .unwrap()
            .promotion_approvals(Some("demo"))
            .unwrap(),
        [complete, saved]
    );
}

#[test]
fn no_policy_is_not_vacuous_readiness_and_external_reporting_does_not_require_a_policy() {
    let fixture = Fixture::new(BASE);
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    let context = context(&app);
    registry
        .record_harness_result(&report(&context, 50, 50))
        .unwrap();
    registry
        .record_consumer_deployment(&deployment(&context, "lab-device"))
        .unwrap();
    let assessment = registry.assess_promotion(&context).unwrap();
    assert_eq!(assessment.status, PromotionStatus::NotConfigured);
    assert!(assessment.criteria.is_empty());
    assert_eq!(
        assessment.stage_requirements,
        app.manifest.stages[1].requires
    );
    assert_eq!(
        registry
            .approve_promotion(&approval(&assessment))
            .unwrap_err()
            .code(),
        "promotion_not_configured"
    );
    assert!(registry.promotion_approvals(None).unwrap().is_empty());
}

#[test]
fn fingerprints_are_stable_but_latest_equal_counts_and_markers_invalidate_review() {
    let (_fixture, registry, app) = setup(&format!("{HARNESS}, {REQUIRED}"));
    let context = context(&app);
    registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    let marker = deployment(&context, "lab-device");
    registry.record_consumer_deployment(&marker).unwrap();
    let first = registry.assess_promotion(&context).unwrap();
    assert_eq!(registry.assess_promotion(&context).unwrap(), first);
    let request = approval(&first);
    registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    let second = registry.assess_promotion(&context).unwrap();
    assert_ne!(second.assessment_hash, first.assessment_hash);
    assert_eq!(second.criteria[0].observed, first.criteria[0].observed);
    assert_eq!(
        registry.approve_promotion(&request).unwrap_err().code(),
        "assessment_changed"
    );
    registry.record_consumer_deployment(&marker).unwrap();
    let third = registry.assess_promotion(&context).unwrap();
    assert_ne!(third.assessment_hash, second.assessment_hash);
    assert_eq!(
        registry
            .approve_promotion(&approval(&second))
            .unwrap_err()
            .code(),
        "assessment_changed"
    );
    registry.approve_promotion(&approval(&third)).unwrap();
    registry
        .record_harness_result(&report(&context, 50, 44))
        .unwrap();
    assert_eq!(
        registry.assess_promotion(&context).unwrap().status,
        PromotionStatus::Blocked
    );
    assert_eq!(
        registry
            .approve_promotion(&approval(&third))
            .unwrap_err()
            .code(),
        "assessment_changed"
    );
}

#[test]
fn unrelated_apps_checks_releases_and_stages_do_not_change_assessment_hash() {
    let (fixture, registry, app) = setup(HARNESS);
    let context = context(&app);
    registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    let expected = registry.assess_promotion(&context).unwrap();
    let mut unrelated = report(&context, 50, 0);
    unrelated.check_id = "extra".to_owned();
    registry.record_harness_result(&unrelated).unwrap();
    unrelated.check_id = "smoke".to_owned();
    unrelated.release_ref = "another-candidate".to_owned();
    registry.record_harness_result(&unrelated).unwrap();
    unrelated.stage_id = "draft".to_owned();
    unrelated.check_id = "elsewhere".to_owned();
    unrelated.release_ref = context.release_ref.clone();
    registry.record_harness_result(&unrelated).unwrap();
    let path = fixture.path("other.yaml");
    fs::write(&path, yaml(HARNESS).replace("app: demo", "app: other")).unwrap();
    let other = registry.register(&path, None, false).unwrap();
    let mut other_request = report(&context, 50, 0);
    other_request.app_id = "other".to_owned();
    other_request.expected_manifest_hash = other.manifest.manifest_hash;
    other_request.expected_updated_at = other.updated_at;
    registry.record_harness_result(&other_request).unwrap();
    registry
        .record_consumer_deployment(&deployment(&context, "personal-device"))
        .unwrap();
    assert_eq!(registry.assess_promotion(&context).unwrap(), expected);
}

#[test]
fn evidence_must_match_every_provenance_field_and_the_current_destination() {
    let (_fixture, registry, app) = setup(&format!("{HARNESS}, {REQUIRED}"));
    let context = context(&app);
    registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    registry
        .record_consumer_deployment(&deployment(&context, "lab-device"))
        .unwrap();
    let connection = database(&registry);
    let original = registry.assess_promotion(&context).unwrap();
    for table in ["harness_results", "consumer_deployments"] {
        let mut changes = vec![
            ("app_id", "foreign".to_owned()),
            ("app_name", "Different app".to_owned()),
            ("stage_id", "draft".to_owned()),
            ("release_ref", "foreign-release".to_owned()),
            ("manifest_hash", "a".repeat(64)),
            ("manifest_path", "/different/manifest.yaml".to_owned()),
            ("source_path", "/different/source".to_owned()),
            ("registration_updated_at", "2000-01-01T00:00:00Z".to_owned()),
        ];
        if table == "harness_results" {
            changes.push(("check_id", "extra".to_owned()));
        } else {
            changes.push(("consumer_id", "personal-device".to_owned()));
            changes.push(("consumer_target", "different target".to_owned()));
        }
        for (field, value) in changes {
            let old: String = connection
                .query_row(&format!("SELECT {field} FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            connection
                .execute(&format!("UPDATE {table} SET {field} = ?1"), [&value])
                .unwrap();
            let assessment = registry.assess_promotion(&context).unwrap();
            assert_eq!(
                assessment.status,
                PromotionStatus::Blocked,
                "{table}.{field}"
            );
            let index = usize::from(table == "consumer_deployments");
            assert!(
                assessment.criteria[index].evidence_id.is_none(),
                "{table}.{field}"
            );
            assert!(assessment.criteria[index].observed.is_none());
            connection
                .execute(&format!("UPDATE {table} SET {field} = ?1"), [old])
                .unwrap();
        }
    }
    assert_eq!(registry.assess_promotion(&context).unwrap(), original);
}

#[test]
fn replacement_path_config_and_reregistration_invalidate_review_and_evidence() {
    let (fixture, registry, app) = setup(HARNESS);
    let old_context = context(&app);
    registry
        .record_harness_result(&report(&old_context, 50, 45))
        .unwrap();
    let old_assessment = registry.assess_promotion(&old_context).unwrap();
    let other_path = fixture.path("same-content.yaml");
    fs::write(&other_path, yaml(HARNESS)).unwrap();
    let moved = registry.register(&other_path, None, true).unwrap();
    assert_eq!(moved.manifest.manifest_hash, app.manifest.manifest_hash);
    assert_ne!(moved.updated_at, app.updated_at);
    assert_eq!(
        registry
            .approve_promotion(&approval(&old_assessment))
            .unwrap_err()
            .code(),
        "registration_changed"
    );
    assert_eq!(
        registry.assess_promotion(&context(&moved)).unwrap().status,
        PromotionStatus::Blocked
    );
    fixture.write(&format!(
        "{}# a changed configuration revision\n",
        yaml(HARNESS)
    ));
    let changed = registry.register(&fixture.manifest, None, true).unwrap();
    let changed_assessment = registry.assess_promotion(&context(&changed)).unwrap();
    assert_ne!(
        changed_assessment.assessment_hash,
        old_assessment.assessment_hash
    );
    registry
        .record_harness_result(&report(&context(&changed), 50, 45))
        .unwrap();
    registry.unregister("demo").unwrap();
    let registered = registry.register(&fixture.manifest, None, false).unwrap();
    assert_ne!(registered.updated_at, changed.updated_at);
    assert_eq!(
        registry
            .assess_promotion(&context(&registered))
            .unwrap()
            .status,
        PromotionStatus::Blocked
    );
}

#[test]
fn reporting_and_approval_work_offline_without_executing_or_marking_legacy_gates_passed() {
    let fixture = Fixture::new(BASE);
    let sentinel = fixture.path("must-not-execute");
    let command = format!("[sh, -c, \"touch '{}'\"]", sentinel.display());
    let yaml = yaml(&format!("{HARNESS}, {REQUIRED}"))
        .replace("[unavailable-harness]", &command)
        .replace("[unavailable-prepare]", &command)
        .replace(
            "lab / applications",
            &format!("touch '{}'", sentinel.display()),
        );
    fixture.write(&yaml);
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    fs::remove_file(&fixture.manifest).unwrap();
    fs::remove_dir(&fixture.source).unwrap();
    let context = context(&app);
    registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    registry
        .record_consumer_deployment(&deployment(&context, "lab-device"))
        .unwrap();
    let assessment = registry.assess_promotion(&context).unwrap();
    assert_eq!(
        assessment.stage_requirements,
        app.manifest.stages[1].requires
    );
    let saved = registry.approve_promotion(&approval(&assessment)).unwrap();
    assert_eq!(registry.get("demo").unwrap(), app);
    registry.unregister("demo").unwrap();
    let reopened = Registry::open(&fixture.state).unwrap();
    assert_eq!(reopened.promotion_approvals(None).unwrap(), [saved]);
    assert_eq!(reopened.harness_results(None).unwrap().len(), 1);
    assert!(!sentinel.exists());
}

#[test]
fn concurrent_reports_and_approvals_are_serialized_or_explicitly_stale() {
    let (_fixture, registry, app) = setup(HARNESS);
    let context = context(&app);
    registry
        .record_harness_result(&report(&context, 50, 45))
        .unwrap();
    let reviewed = registry.assess_promotion(&context).unwrap();
    let count = 8;
    let barrier = Arc::new(Barrier::new(count + 1));
    let reporter = {
        let registry = registry.clone();
        let barrier = barrier.clone();
        let request = report(&context, 50, 0);
        thread::spawn(move || {
            barrier.wait();
            registry.record_harness_result(&request)
        })
    };
    let approvals: Vec<_> = (0..count)
        .map(|_| {
            let registry = registry.clone();
            let barrier = barrier.clone();
            let request = approval(&reviewed);
            thread::spawn(move || {
                barrier.wait();
                registry.approve_promotion(&request)
            })
        })
        .collect();
    reporter.join().unwrap().unwrap();
    let mut accepted = Vec::new();
    for thread in approvals {
        match thread.join().unwrap() {
            Ok(saved) => {
                assert_eq!(saved.assessment, reviewed);
                accepted.push(saved);
            }
            Err(error) => assert_eq!(error.code(), "assessment_changed"),
        }
    }
    let saved = registry.promotion_approvals(None).unwrap();
    assert_eq!(saved.len(), accepted.len());
    assert!(accepted.iter().all(|approval| saved.contains(approval)));
    assert_eq!(registry.harness_results(None).unwrap().len(), 2);
    assert_eq!(
        registry.assess_promotion(&context).unwrap().status,
        PromotionStatus::Blocked
    );
    assert_eq!(registry.get("demo").unwrap(), app);
}
