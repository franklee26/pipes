mod support;

use std::{fs, num::NonZeroU32};

use pipes_core::{CheckPlanNode, ManifestPreview, Parallelism, Registry, preview_manifest};
use rusqlite::Connection;
use serde_json::{Value, json};
use support::{FULL, Fixture, MINIMAL, canonical};

fn with_checks(checks: &str) -> String {
    MINIMAL.replace(
        "stages: [{id: build}]",
        &format!("stages: [{{id: build, checks: {checks}, require: [checks.passed]}}]"),
    )
}

fn command(id: &str) -> Value {
    json!({
        "id": id, "exec": ["test-tool", id], "cwd": null, "timeout": null,
        "env": {}, "secrets": {}
    })
}

fn leaf(id: &str) -> Value {
    json!({"kind": "command", "command": command(id)})
}

fn batches() -> String {
    with_checks(
        r#"[
  {id: batch-a, parallel: [
    {id: a-one, exec: [test-tool, a-one]},
    {id: a-two, exec: [test-tool, a-two]}
  ], max_parallel: 8},
  {id: batch-b, parallel: [
    {id: b-one, exec: [test-tool, b-one]},
    {id: b-two, exec: [test-tool, b-two]}
  ]}
]"#,
    )
}

fn expected_batches() -> Value {
    json!([
        {"kind": "parallel", "id": "batch-a", "steps": [leaf("a-one"), leaf("a-two")], "maxParallel": 8},
        {"kind": "parallel", "id": "batch-b", "steps": [leaf("b-one"), leaf("b-two")], "maxParallel": null}
    ])
}

#[test]
fn parallel_batches_remain_sequential_roots_with_exact_canonical_and_flattened_shapes() {
    let fixture = Fixture::new(&format!("{}execution: {{max_parallel: 4}}\n", batches()));
    let preview = preview_manifest(&fixture.manifest).unwrap();
    let stage = &preview.stages[0];
    assert_eq!(stage.requires, ["checks.passed"]);
    let json = serde_json::to_value(&preview).unwrap();
    assert_eq!(json["execution"]["maxParallel"], 4);
    assert_eq!(json["stages"][0]["checkPlan"], expected_batches());
    assert_eq!(
        json["stages"][0]["checks"],
        json!([
            command("a-one"),
            command("a-two"),
            command("b-one"),
            command("b-two")
        ])
    );
    assert_eq!(
        serde_json::from_value::<ManifestPreview>(json).unwrap(),
        preview
    );
    assert!(!fixture.state.exists());
}

#[test]
fn mixed_nested_plans_preserve_every_barrier_and_depth_first_leaf_order() {
    let yaml = with_checks(
        r#"[
  {id: preflight, exec: [test-tool, preflight]},
  {id: schedule, sequence: [
    {id: batch-a, parallel: [
      {id: variant, sequence: [
        {id: setup, exec: [test-tool, setup]},
        {id: run, exec: [test-tool, run]}
      ]},
      {id: other, exec: [test-tool, other]}
    ], max_parallel: 6},
    {id: batch-b, parallel: [
      {id: b-first, exec: [test-tool, b-first]},
      {id: nested, parallel: [
        {id: nested-one, exec: [test-tool, nested-one]},
        {id: nested-two, exec: [test-tool, nested-two]}
      ], max_parallel: 12}
    ], max_parallel: 2}
  ]},
  {id: postflight, exec: [test-tool, postflight]}
]"#,
    );
    let fixture = Fixture::new(&format!("{yaml}execution: {{max_parallel: 3}}\n"));
    let preview = preview_manifest(&fixture.manifest).unwrap();
    let ids: Vec<_> = preview.stages[0]
        .checks
        .iter()
        .map(|check| check.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "preflight",
            "setup",
            "run",
            "other",
            "b-first",
            "nested-one",
            "nested-two",
            "postflight"
        ]
    );
    let expected = json!([
        leaf("preflight"),
        {"kind": "sequence", "id": "schedule", "steps": [
            {"kind": "parallel", "id": "batch-a", "maxParallel": 6, "steps": [
                {"kind": "sequence", "id": "variant", "steps": [leaf("setup"), leaf("run")]},
                leaf("other")
            ]},
            {"kind": "parallel", "id": "batch-b", "maxParallel": 2, "steps": [
                leaf("b-first"),
                {"kind": "parallel", "id": "nested", "maxParallel": 12, "steps": [
                    leaf("nested-one"), leaf("nested-two")
                ]}
            ]}
        ]},
        leaf("postflight")
    ]);
    assert_eq!(
        serde_json::to_value(&preview.stages[0].check_plan).unwrap(),
        expected
    );
    assert_eq!(
        serde_json::to_value(preview.execution.max_parallel).unwrap(),
        3
    );
}

#[test]
fn flat_legacy_checks_become_command_nodes_and_auto_is_not_resolved() {
    let fixture = Fixture::new(FULL);
    let preview = preview_manifest(&fixture.manifest).unwrap();
    assert_eq!(preview.manifest_path, canonical(&fixture.manifest));
    assert_eq!(preview.execution.max_parallel, Parallelism::Auto);
    assert_eq!(
        preview.stages[0].check_plan,
        [CheckPlanNode::Command {
            command: preview.stages[0].checks[0].clone()
        }]
    );
    assert!(preview.stages[1].check_plan.is_empty());
    assert_eq!(preview.prepare[0].id, "version");
}

#[test]
fn one_child_groups_and_inherited_caps_are_valid() {
    let fixture = Fixture::new(&with_checks(
        "[{id: outer, sequence: [{id: inner, parallel: [{id: one, exec: [test-tool, one]}]}]}]",
    ));
    let preview = preview_manifest(&fixture.manifest).unwrap();
    assert_eq!(preview.stages[0].checks.len(), 1);
    let plan = serde_json::to_value(&preview.stages[0].check_plan).unwrap();
    assert_eq!(plan[0]["steps"][0]["maxParallel"], Value::Null);
    assert_eq!(preview.execution.max_parallel, Parallelism::Auto);
}

#[test]
fn parallelism_json_is_only_auto_or_a_positive_u32() {
    for value in [json!("auto"), json!(1), json!(4294967295_u32)] {
        let budget: Parallelism = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(budget).unwrap(), value);
    }
    let limit = Parallelism::Limit(NonZeroU32::new(7).unwrap());
    assert_eq!(serde_json::to_value(limit).unwrap(), 7);
    for value in [
        Value::Null,
        json!(0),
        json!(-1),
        json!(1.5),
        json!(4294967296_u64),
        json!("2"),
        json!("AUTO"),
        json!(true),
        json!({}),
        json!([]),
    ] {
        assert!(serde_json::from_value::<Parallelism>(value).is_err());
    }
}

#[test]
fn yaml_budgets_are_strict_and_group_limits_may_exceed_the_run_budget() {
    let fixture = Fixture::new(MINIMAL);
    for (value, expected) in [
        ("auto", json!("auto")),
        ("1", json!(1)),
        ("4294967295", json!(4294967295_u32)),
    ] {
        fixture.write(&format!("{MINIMAL}execution: {{max_parallel: {value}}}\n"));
        let preview = preview_manifest(&fixture.manifest).unwrap();
        assert_eq!(
            serde_json::to_value(preview.execution.max_parallel).unwrap(),
            expected
        );
    }
    for value in [
        "null",
        "~",
        "",
        "0",
        "-1",
        "1.5",
        "'2'",
        "'AUTO'",
        "4294967296",
        "18446744073709551616",
        "true",
        "[]",
        "{}",
    ] {
        fixture.write(&format!("{MINIMAL}execution: {{max_parallel: {value}}}\n"));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest", "{value}");
        assert!(error.to_string().contains("max_parallel"), "{error}");
    }
    fixture.write(&format!(
        "{}execution: {{max_parallel: 1}}\n",
        with_checks(
            "[{id: batch, parallel: [{id: one, exec: [test-tool, one]}], max_parallel: 4294967295}]"
        )
    ));
    let json = serde_json::to_value(preview_manifest(&fixture.manifest).unwrap()).unwrap();
    assert_eq!(json["execution"]["maxParallel"], 1);
    assert_eq!(
        json["stages"][0]["checkPlan"][0]["maxParallel"],
        4294967295_u32
    );
}

#[test]
fn every_invalid_group_cap_is_rejected_without_coercion() {
    let fixture = Fixture::new(MINIMAL);
    for value in [
        "auto",
        "null",
        "~",
        "",
        "0",
        "-1",
        "0.5",
        "'2'",
        "4294967296",
        "true",
        "[]",
        "{}",
    ] {
        fixture.write(&with_checks(&format!(
            "[{{id: batch, parallel: [{{id: one, exec: [test-tool, one]}}], max_parallel: {value}}}]"
        )));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest", "{value}");
        assert!(error.to_string().contains("max_parallel"), "{error}");
    }
}

#[test]
fn node_kinds_are_exclusive_and_groups_are_nonempty_non_null_lists() {
    let fixture = Fixture::new(MINIMAL);
    for node in [
        "{id: missing}",
        "{id: mixed, exec: [tool], sequence: [{id: one, exec: [tool]}]}",
        "{id: mixed, exec: [tool], parallel: [{id: one, exec: [tool]}]}",
        "{id: mixed, parallel: [{id: one, exec: [tool]}], sequence: [{id: two, exec: [tool]}]}",
        "{id: empty, sequence: []}",
        "{id: empty, parallel: []}",
        "{id: empty, sequence: null}",
        "{id: empty, parallel: null}",
        "{id: empty, sequence: }",
        "{id: empty, parallel: }",
        "{id: wrong, sequence: {id: one, exec: [tool]}}",
        "{id: wrong, parallel: text}",
        "{id: wrong, parallel: [true]}",
        "{id: wrong, parallel: [[{id: one, exec: [tool]}]]}",
        "{id: Bad_Name, parallel: [{id: one, exec: [tool]}]}",
        "{id: group, parallel: [{id: one, exec: [tool]}], allow_failure: true}",
        "{id: group, sequence: [{id: one, exec: [tool]}], any_success: true}",
        "{id: one, exec: [tool], skip: true}",
    ] {
        fixture.write(&with_checks(&format!("[{node}]")));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest", "{node}");
        assert!(error.to_string().contains("stages"), "{error}");
        assert!(
            error.to_string().contains("line") || error.to_string().contains(".checks"),
            "{error}"
        );
    }
}

#[test]
fn kind_specific_fields_cannot_be_smuggled_onto_other_nodes() {
    let fixture = Fixture::new(MINIMAL);
    for (node, field) in [
        ("{id: one, exec: [tool], max_parallel: 2}", "max_parallel"),
        (
            "{id: group, sequence: [{id: one, exec: [tool]}], max_parallel: 2}",
            "max_parallel",
        ),
        (
            "{id: group, parallel: [{id: one, exec: [tool]}], cwd: .}",
            "cwd",
        ),
        (
            "{id: group, sequence: [{id: one, exec: [tool]}], cwd: null}",
            "cwd",
        ),
        (
            "{id: group, parallel: [{id: one, exec: [tool]}], timeout: 1s}",
            "timeout",
        ),
        (
            "{id: group, sequence: [{id: one, exec: [tool]}], timeout: null}",
            "timeout",
        ),
        (
            "{id: group, parallel: [{id: one, exec: [tool]}], env: {}}",
            "env",
        ),
        (
            "{id: group, sequence: [{id: one, exec: [tool]}], secrets: {}}",
            "secrets",
        ),
    ] {
        fixture.write(&with_checks(&format!("[{node}]")));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest");
        assert!(error.to_string().contains(field), "{error}");
    }
}

#[test]
fn node_ids_are_unique_across_the_entire_stage_not_just_siblings() {
    let fixture = Fixture::new(MINIMAL);
    for checks in [
        "[{id: group, parallel: [{id: group, exec: [tool]}]}]",
        "[{id: a, parallel: [{id: same, exec: [tool]}]}, {id: b, sequence: [{id: same, exec: [tool]}]}]",
        "[{id: group, sequence: [{id: one, exec: [tool]}]}, {id: group, parallel: [{id: two, exec: [tool]}]}]",
    ] {
        fixture.write(&with_checks(checks));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest");
        assert!(
            error.to_string().contains("duplicate check node ID"),
            "{error}"
        );
    }
    fixture.write(&MINIMAL.replace(
        "stages: [{id: build}]",
        "prepare: [{id: one, exec: [tool]}]\n\
         stages: [{id: first, checks: [{id: group, sequence: [{id: one, exec: [tool]}]}]}, \
                  {id: second, checks: [{id: group, sequence: [{id: one, exec: [tool]}]}]}]",
    ));
    assert!(preview_manifest(&fixture.manifest).is_ok());
}

#[test]
fn nested_commands_reuse_the_original_command_validation_and_field_paths() {
    let fixture = Fixture::new(MINIMAL);
    for (fields, location) in [
        ("exec: []", ".exec"),
        ("exec: [true]", ".exec"),
        ("exec: [tool], cwd: ''", ".cwd"),
        ("exec: [tool], timeout: 0s", ".timeout"),
        ("exec: [tool], env: {BAD-NAME: value}", ".env."),
        (
            "exec: [tool], secrets: {TOKEN: literal-placeholder}",
            ".secrets.",
        ),
        (
            "exec: [tool], env: {TOKEN: value}, secrets: {TOKEN: 'env://TOKEN'}",
            ".secrets.",
        ),
        (r#"exec: [tool, "a\0b"]"#, ".exec"),
    ] {
        fixture.write(&with_checks(&format!(
            "[{{id: batch, parallel: [{{id: nested, {fields}}}]}}]"
        )));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest");
        assert!(error.to_string().contains(location), "{error}");
        assert!(error.to_string().contains("parallel[0]"), "{error}");
    }
}

#[test]
fn duplicate_yaml_fields_and_map_keys_are_not_overwritten() {
    let fixture = Fixture::new(MINIMAL);
    for node in [
        "{id: a, id: b, parallel: [{id: one, exec: [tool]}]}",
        "{id: a, parallel: [{id: one, exec: [tool]}], parallel: [{id: two, exec: [tool]}]}",
        "{id: a, parallel: [{id: one, exec: [tool]}], max_parallel: 1, max_parallel: 2}",
        "{id: a, parallel: [{id: one, exec: [tool], env: {A: a, A: b}}]}",
    ] {
        fixture.write(&with_checks(&format!("[{node}]")));
        assert!(
            preview_manifest(&fixture.manifest)
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
    }
}

#[test]
fn checks_passed_needs_a_leaf_and_prepare_remains_flat() {
    let fixture = Fixture::new(&with_checks("[]"));
    let error = preview_manifest(&fixture.manifest).unwrap_err();
    assert_eq!(error.code(), "invalid_manifest");
    assert!(
        error
            .to_string()
            .contains("checks.passed needs at least one check")
    );
    for kind in ["sequence", "parallel"] {
        fixture.write(&format!(
            "{MINIMAL}prepare: [{{id: group, {kind}: [{{id: one, exec: [tool]}}]}}]\n"
        ));
        assert!(
            preview_manifest(&fixture.manifest)
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );
    }
}

fn nested_node(levels: usize) -> String {
    let mut node = "{id: leaf, exec: [test-tool, leaf]}".to_owned();
    for index in 0..levels {
        let kind = if index % 2 == 0 {
            "parallel"
        } else {
            "sequence"
        };
        node = format!("{{id: group-{index}, {kind}: [{node}]}}");
    }
    node
}

#[test]
fn exactly_32_group_levels_are_supported_and_33_are_rejected_before_expansion() {
    let fixture = Fixture::new(&with_checks(&format!("[{}]", nested_node(32))));
    let preview = preview_manifest(&fixture.manifest).unwrap();
    assert_eq!(preview.stages[0].checks.len(), 1);
    let mut nodes = preview.stages[0].check_plan.as_slice();
    let mut levels = 0;
    loop {
        match &nodes[0] {
            CheckPlanNode::Command { command } => {
                assert_eq!(command.id, "leaf");
                break;
            }
            CheckPlanNode::Sequence { steps, .. } | CheckPlanNode::Parallel { steps, .. } => {
                levels += 1;
                nodes = steps;
            }
        }
    }
    assert_eq!(levels, 32);
    let encoded = serde_json::to_string(&preview).unwrap();
    assert_eq!(
        serde_json::from_str::<ManifestPreview>(&encoded).unwrap(),
        preview
    );
    let registry = Registry::open(&fixture.state).unwrap();
    registry.register(&fixture.manifest, None, false).unwrap();
    assert_eq!(registry.get("demo").unwrap().manifest, preview);
    for levels in [33, 128, 1024] {
        fixture.write(&with_checks(&format!("[{}]", nested_node(levels))));
        let error = preview_manifest(&fixture.manifest).unwrap_err();
        assert_eq!(error.code(), "invalid_manifest");
        assert!(error.to_string().contains("32 levels"), "{error}");
    }
}

#[test]
fn recursive_alias_plans_fail_without_unbounded_construction() {
    let fixture = Fixture::new(&with_checks("[&loop {id: loop, parallel: [*loop]}]"));
    let error = preview_manifest(&fixture.manifest).unwrap_err();
    assert_eq!(error.code(), "invalid_manifest");
    assert!(
        error.to_string().contains("32 levels") || error.to_string().contains("recursion"),
        "{error}"
    );
}

#[test]
fn aliased_plan_expansion_is_bounded_before_duplicate_id_validation() {
    let mut nodes = format!(
        "&level0 {{id: leaf, exec: [tool, '{}']}}",
        "x".repeat(64 * 1024)
    );
    for level in 1..20 {
        nodes.push_str(&format!(
            ", &level{level} {{id: group-{level}, parallel: [*level{}, *level{}]}}",
            level - 1,
            level - 1,
        ));
    }
    let yaml = with_checks(&format!("[{nodes}]"));
    assert!(yaml.len() < 1024 * 1024);
    let fixture = Fixture::new(&yaml);
    let error = preview_manifest(&fixture.manifest).unwrap_err();
    assert_eq!(error.code(), "invalid_manifest");
    assert!(
        error.to_string().contains("budget") || error.to_string().contains("repetition"),
        "{error}"
    );
}

#[test]
fn benign_command_aliases_across_stages_remain_supported() {
    let yaml = MINIMAL.replace(
        "stages: [{id: build}]",
        "prepare: [&shared {id: same, exec: [tool]}]\n\
         stages: [{id: first, checks: [*shared]}, {id: second, checks: [*shared]}]",
    );
    let fixture = Fixture::new(&yaml);
    let preview = preview_manifest(&fixture.manifest).unwrap();
    assert_eq!(preview.stages[0].checks[0], preview.prepare[0]);
    assert_eq!(preview.stages[1].check_plan, preview.stages[0].check_plan);
}

#[test]
fn public_plan_json_is_strict_and_required_fields_do_not_default() {
    let good =
        json!({"kind": "parallel", "id": "batch", "steps": [leaf("one")], "maxParallel": null});
    let node: CheckPlanNode = serde_json::from_value(good.clone()).unwrap();
    assert_eq!(serde_json::to_value(node.clone()).unwrap(), good);
    assert_eq!(node, serde_json::from_value(good.clone()).unwrap());
    for field in ["maxParallel", "steps", "id"] {
        let mut value = good.clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<CheckPlanNode>(value).is_err(),
            "{field}"
        );
    }
    for value in [
        json!({"kind": "parallel", "id": "p", "steps": [leaf("one")], "max_parallel": 2}),
        json!({"kind": "parallel", "id": "p", "steps": [leaf("one")], "maxParallel": 0}),
        json!({"kind": "sequence", "id": "s", "steps": [leaf("one")], "maxParallel": null}),
        json!({"kind": "command", "command": command("one"), "id": "extra"}),
        json!({"kind": "any", "id": "a", "steps": [leaf("one")]}),
    ] {
        assert!(serde_json::from_value::<CheckPlanNode>(value).is_err());
    }
    let fixture = Fixture::new(MINIMAL);
    let original = serde_json::to_value(preview_manifest(&fixture.manifest).unwrap()).unwrap();
    let mut missing = original.clone();
    missing["execution"]
        .as_object_mut()
        .unwrap()
        .remove("maxParallel");
    assert!(serde_json::from_value::<ManifestPreview>(missing).is_err());
    let mut missing = original;
    missing["stages"][0]
        .as_object_mut()
        .unwrap()
        .remove("checkPlan");
    assert!(serde_json::from_value::<ManifestPreview>(missing).is_err());
}

#[test]
fn plan_snapshots_round_trip_offline_and_never_execute_nested_commands() {
    let fixture = Fixture::new(&batches());
    let marker = fixture.path("MUST-NOT-EXIST");
    let script = format!("touch '{}'", marker.display());
    let exec = serde_json::to_string(&["sh", "-c", script.as_str()]).unwrap();
    fixture.write(&batches().replace("[test-tool, a-one]", &exec));
    let preview = preview_manifest(&fixture.manifest).unwrap();
    let registry = Registry::open(&fixture.state).unwrap();
    let registered = registry
        .register(&fixture.manifest, Some(&preview.manifest_hash), false)
        .unwrap();
    assert_eq!(registered.manifest, preview);
    fs::remove_file(&fixture.manifest).unwrap();
    fs::remove_dir(&fixture.source).unwrap();
    assert_eq!(
        Registry::open(&fixture.state).unwrap().get("demo").unwrap(),
        registered
    );
    assert_eq!(
        registry.list().unwrap().as_slice(),
        std::slice::from_ref(&registered)
    );
    registry.unregister("demo").unwrap();
    assert!(!marker.exists());
}

#[test]
fn current_schema_detects_tree_flattening_and_budget_corruption_without_repair() {
    let fixture = Fixture::new(&batches());
    let registry = Registry::open(&fixture.state).unwrap();
    let app = registry.register(&fixture.manifest, None, false).unwrap();
    let original = serde_json::to_value(&app.manifest).unwrap();
    let connection = Connection::open(registry.data_dir().join("registry.sqlite3")).unwrap();
    let mut corruptions = Vec::new();
    let mut value = original.clone();
    value["execution"]
        .as_object_mut()
        .unwrap()
        .remove("maxParallel");
    corruptions.push(value);
    let mut value = original.clone();
    value["execution"]["maxParallel"] = json!(3);
    corruptions.push(value);
    let mut value = original.clone();
    value["stages"][0]
        .as_object_mut()
        .unwrap()
        .remove("checkPlan");
    corruptions.push(value);
    let mut value = original.clone();
    value["stages"][0]["checks"]
        .as_array_mut()
        .unwrap()
        .reverse();
    corruptions.push(value);
    let mut value = original.clone();
    value["stages"][0]["checkPlan"][0]["steps"]
        .as_array_mut()
        .unwrap()
        .reverse();
    corruptions.push(value);
    let mut value = original.clone();
    value["stages"][0]["checkPlan"][0]["maxParallel"] = json!(2);
    corruptions.push(value);
    let mut value = original.clone();
    value["stages"][0]["checkPlan"][1]
        .as_object_mut()
        .unwrap()
        .remove("maxParallel");
    corruptions.push(value);
    let mut value = original.clone();
    value["stages"][0]["checkPlan"][0] = json!({
        "kind": "sequence", "id": "batch-a", "steps": [leaf("a-one"), leaf("a-two")]
    });
    corruptions.push(value);
    for value in corruptions {
        let snapshot = value.to_string();
        connection
            .execute("UPDATE registered_apps SET snapshot = ?1", [&snapshot])
            .unwrap();
        assert_eq!(registry.get("demo").unwrap_err().code(), "storage");
        assert_eq!(registry.list().unwrap_err().code(), "storage");
        let saved: String = connection
            .query_row("SELECT snapshot FROM registered_apps", [], |row| row.get(0))
            .unwrap();
        assert_eq!(saved, snapshot);
    }
}
