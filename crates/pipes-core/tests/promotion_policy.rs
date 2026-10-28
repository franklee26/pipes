mod promotion;
#[allow(dead_code)]
mod support;

use pipes_core::{ManifestPreview, PromotionCriterion, Registry, preview_manifest};
use promotion::*;
use serde_json::{Value, json};
use support::Fixture;

#[test]
fn omitted_policy_is_empty_but_present_policy_must_be_nonempty() {
    let fixture = Fixture::new(BASE);
    let preview = preview_manifest(&fixture.manifest).unwrap();
    assert!(
        preview
            .stages
            .iter()
            .all(|stage| stage.promotion_criteria.is_empty())
    );
    assert_eq!(
        serde_json::to_value(&preview).unwrap()["stages"][1]["promotionCriteria"],
        json!([])
    );
    for policy in [
        "null",
        "~",
        "",
        "{}",
        "{criteria: []}",
        "{criteria: null}",
        "{criteria:}",
        "{criteria: ~}",
    ] {
        fixture.write(&yaml_with_promotion(policy));
        assert_eq!(
            preview_manifest(&fixture.manifest).unwrap_err().code(),
            "invalid_manifest",
            "{policy}"
        );
    }
}

#[test]
fn promotion_json_is_camel_case_and_does_not_change_any_existing_plan_semantics() {
    let fixture = Fixture::new(BASE);
    let original = preview_manifest(&fixture.manifest).unwrap();
    fixture.write(&yaml(&format!("{HARNESS}, {REQUIRED}, {OPTIONAL}")));
    let mut preview = preview_manifest(&fixture.manifest).unwrap();
    let criteria = std::mem::take(&mut preview.stages[1].promotion_criteria);
    assert_eq!(
        serde_json::to_value(&criteria).unwrap(),
        json!([
            {"id": "smoke-tests", "kind": "harness", "checkId": "smoke", "expectedTests": 50, "minimumPassed": 45},
            {"id": "lab-delivery", "kind": "deployment", "consumerId": "lab-device", "requirement": "required"},
            {"id": "personal-delivery", "kind": "deployment", "consumerId": "personal-device", "requirement": "optional"}
        ])
    );
    assert_eq!(preview.stages, original.stages);
    assert_eq!(preview.execution, original.execution);
    assert_eq!(preview.prepare, original.prepare);
    assert_eq!(preview.outputs, original.outputs);
    assert_eq!(preview.stages.len(), 3);
    assert_eq!(preview.stages[2].after, ["tested"]);
    assert!(!fixture.state.exists());
}

#[test]
fn policy_fields_kinds_counts_and_requirements_are_strict() {
    let fixture = Fixture::new(BASE);
    let mut invalid = vec![
        "null".to_owned(),
        "[]".to_owned(),
        "{}".to_owned(),
        HARNESS.replace("kind: harness", "kind: unknown"),
        HARNESS.replace("kind: harness", "kind: Harness"),
        HARNESS.replace("kind: harness", "kind: null"),
        HARNESS.replace("id: smoke-tests", "id: null"),
        HARNESS.replace("id: smoke-tests", "id: true"),
        HARNESS.replace("check: smoke", "check: 123"),
        HARNESS.replace("check: smoke", "check: null"),
        HARNESS.replace("check: smoke", "checkId: smoke"),
        HARNESS.replace("expected_tests: 50", "expectedTests: 50"),
        HARNESS.replace(", expected_tests: 50", ""),
        HARNESS.replace(", minimum_passed: 45", ""),
        HARNESS.replace("expected_tests: 50", "percentage: 90"),
        HARNESS.replace("expected_tests: 50", "expected_tests: 50, total: 50"),
        HARNESS.replace(
            "expected_tests: 50",
            "expected_tests: 50, expected_tests: 50",
        ),
        HARNESS.replace(
            "expected_tests: 50",
            "expected_tests: 50, requirement: optional",
        ),
        REQUIRED.replace(", requirement: required", ""),
        REQUIRED.replace("requirement: required", "requirement: null"),
        REQUIRED.replace("requirement: required", "requirement: Required"),
        REQUIRED.replace("requirement: required", "requirement: auto"),
        REQUIRED.replace("requirement: required", "requirement: true"),
        REQUIRED.replace("requirement: required", "requirement: {required: null}"),
        REQUIRED.replace("consumer: lab-device", "consumer: null"),
        REQUIRED.replace("consumer: lab-device", "consumer: true"),
        REQUIRED.replace("consumer: lab-device", "consumer: lab-device, check: smoke"),
    ];
    for value in [
        "0",
        "-1",
        "1.5",
        ".nan",
        ".inf",
        "true",
        "'50'",
        "null",
        "[]",
        "{}",
        "4294967296",
        "18446744073709551616",
    ] {
        invalid.push(HARNESS.replace("expected_tests: 50", &format!("expected_tests: {value}")));
        invalid.push(HARNESS.replace("minimum_passed: 45", &format!("minimum_passed: {value}")));
    }
    invalid.push(HARNESS.replace("minimum_passed: 45", "minimum_passed: 51"));
    for criterion in invalid {
        fixture.write(&yaml(&criterion));
        assert_eq!(
            preview_manifest(&fixture.manifest).unwrap_err().code(),
            "invalid_manifest",
            "{criterion}"
        );
    }
    for promotion in [
        "{criteria: [true]}",
        "{criteria: [], unknown: true}",
        "{criteria: [], criteria: []}",
        "false",
        "42",
        "[]",
    ] {
        fixture.write(&yaml_with_promotion(promotion));
        assert_eq!(
            preview_manifest(&fixture.manifest).unwrap_err().code(),
            "invalid_manifest",
            "{promotion}"
        );
    }
    fixture.write(&yaml(
        &HARNESS
            .replace("50", "4294967295")
            .replace("45", "4294967295"),
    ));
    assert!(matches!(
        preview_manifest(&fixture.manifest).unwrap().stages[1].promotion_criteria[0],
        PromotionCriterion::Harness {
            expected_tests: u32::MAX,
            minimum_passed: u32::MAX,
            ..
        }
    ));
}

#[test]
fn identifiers_and_references_are_unique_and_scoped_to_command_leaves_and_local_consumers() {
    let fixture = Fixture::new(&yaml(HARNESS));
    assert!(preview_manifest(&fixture.manifest).is_ok());
    for id in ["", "Bad", "a--b", "a_b", "-a", "a-", "a/b", "\u{e9}"] {
        fixture.write(&yaml(
            &HARNESS.replace("id: smoke-tests", &format!("id: {id:?}")),
        ));
        assert_eq!(
            preview_manifest(&fixture.manifest).unwrap_err().code(),
            "invalid_manifest",
            "{id}"
        );
    }
    for check in ["batch", "ordered", "setup", "elsewhere", "not-declared"] {
        fixture.write(&yaml(
            &HARNESS.replace("check: smoke", &format!("check: {check}")),
        ));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert!(error.to_string().contains("command leaf"), "{error}");
    }
    for consumer in ["foreign", "smoke", "released", "not-declared"] {
        fixture.write(&yaml(&REQUIRED.replace("lab-device", consumer)));
        assert_eq!(
            preview_manifest(&fixture.manifest).unwrap_err().code(),
            "invalid_manifest"
        );
    }
    for criteria in [
        format!("{HARNESS}, {HARNESS}"),
        format!("{HARNESS}, {}", HARNESS.replace("smoke-tests", "other")),
        format!("{REQUIRED}, {}", REQUIRED.replace("lab-delivery", "other")),
        format!(
            "{HARNESS}, {}",
            REQUIRED.replace("lab-delivery", "smoke-tests")
        ),
    ] {
        fixture.write(&yaml(&criteria));
        assert!(
            preview_manifest(&fixture.manifest)
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
    }
    fixture.write(&yaml(&format!("{HARNESS}, {REQUIRED}")).replace(
        "  - id: draft\n",
        "  - id: draft\n    promotion: {criteria: [{id: smoke-tests, kind: harness, check: elsewhere, expected_tests: 1, minimum_passed: 1}]}\n",
    ));
    assert!(preview_manifest(&fixture.manifest).is_ok());
}

#[test]
fn current_snapshot_requires_policy_data_and_checks_it_against_saved_yaml() {
    let (fixture, registry, app) = setup(HARNESS);
    let connection = database(&registry);
    let original = serde_json::to_value(&app.manifest).unwrap();
    let mut corruptions = Vec::new();
    let mut missing = original.clone();
    missing["stages"][1]
        .as_object_mut()
        .unwrap()
        .remove("promotionCriteria");
    assert!(serde_json::from_value::<ManifestPreview>(missing.clone()).is_err());
    corruptions.push(missing);
    for value in [
        Value::Null,
        json!([]),
        json!([{"id": "missing", "kind": "harness"}]),
    ] {
        let mut corrupt = original.clone();
        corrupt["stages"][1]["promotionCriteria"] = value;
        corruptions.push(corrupt);
    }
    for (field, value) in [
        ("minimumPassed", json!(44)),
        ("expectedTests", json!(49)),
        ("checkId", json!("extra")),
        ("extra", json!(true)),
    ] {
        let mut corrupt = original.clone();
        corrupt["stages"][1]["promotionCriteria"][0][field] = value;
        corruptions.push(corrupt);
    }
    for corrupt in corruptions {
        let snapshot = corrupt.to_string();
        connection
            .execute("UPDATE registered_apps SET snapshot = ?1", [&snapshot])
            .unwrap();
        assert_eq!(registry.get("demo").unwrap_err().code(), "storage");
        assert_eq!(
            registry
                .assess_promotion(&context(&app))
                .unwrap_err()
                .code(),
            "storage"
        );
        let unchanged: String = connection
            .query_row("SELECT snapshot FROM registered_apps", [], |row| row.get(0))
            .unwrap();
        assert_eq!(unchanged, snapshot);
    }
    connection
        .execute(
            "UPDATE registered_apps SET snapshot = ?1",
            [original.to_string()],
        )
        .unwrap();
    assert_eq!(
        Registry::open(&fixture.state).unwrap().get("demo").unwrap(),
        app
    );
}
