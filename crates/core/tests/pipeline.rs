use merc_core::JobSpec;
use merc_core::JobSpecError;
use merc_core::Tool;
use merc_core::resolve;

const ABP_PIPELINE_JSON: &str = include_str!("../../../examples/abp.json");

/// Wraps `steps` (a JSON array of argv arrays) into a spec with one input,
/// `spec`, and no kept outputs.
fn spec_with_steps(steps: &str) -> JobSpec {
    let json = format!(r#"{{ "schema_version": 2, "inputs": {{ "spec": "abp.mcrl2" }}, "steps": {steps} }}"#);
    JobSpec::from_json(&json).expect("valid spec")
}

#[test]
fn resolves_the_abp_pipeline_end_to_end() {
    let spec = JobSpec::from_json(ABP_PIPELINE_JSON).expect("valid spec");
    let plan = resolve(&spec).expect("valid pipeline");

    assert_eq!(plan.steps.len(), 3);

    let lps = &plan.steps[0];
    assert_eq!(lps.tool, Tool::Mcrl22Lps);
    assert_eq!(lps.args, vec!["-v", "abp.mcrl2", "model.lps"]);
    assert_eq!(lps.outputs, vec!["model.lps"]);

    let explore = &plan.steps[1];
    assert_eq!(explore.tool, Tool::MercLps);
    assert_eq!(explore.args, vec!["explore-explicit", "model.lps", "--strategy", "bfs", "--output", "model.lts"]);
    assert_eq!(explore.outputs, vec!["model.lts"]);

    let reduce = &plan.steps[2];
    assert_eq!(reduce.tool, Tool::MercLts);
    assert_eq!(reduce.args, vec!["reduce", "branching-bisim", "model.lts", "--output", "model.reduced.lts"]);

    assert_eq!(plan.keep, vec!["model.reduced.lts"]);
}

#[test]
fn resolves_references_embedded_in_an_argument() {
    let spec = spec_with_steps(r#"[["merc-lts", "--input=$spec", "--output=$out.a.lts", "100$$"]]"#);
    let plan = resolve(&spec).expect("valid pipeline");
    assert_eq!(plan.steps[0].args, vec!["--input=abp.mcrl2", "--output=a.lts", "100$"]);
}

#[test]
fn rejects_duplicate_output_across_steps() {
    let spec = spec_with_steps(r#"[["merc-lts", "$out.a.lts"], ["merc-lts", "$out.a.lts"]]"#);
    assert!(matches!(resolve(&spec), Err(JobSpecError::DuplicateName { step: 2, name }) if name == "a.lts"));
}

#[test]
fn rejects_output_shadowing_an_input() {
    let spec = spec_with_steps(r#"[["merc-lts", "$out.spec"]]"#);
    assert!(matches!(resolve(&spec), Err(JobSpecError::DuplicateName { step: 1, name }) if name == "spec"));
}

#[test]
fn rejects_unknown_name() {
    let spec = spec_with_steps(r#"[["merc-lts", "$missing"]]"#);
    assert!(matches!(resolve(&spec), Err(JobSpecError::UnknownName { step: 1, name }) if name == "missing"));
}

#[test]
fn rejects_forward_reference() {
    let spec = spec_with_steps(r#"[["merc-lts", "$x.lts"], ["merc-lts", "$out.x.lts"]]"#);
    assert!(matches!(resolve(&spec), Err(JobSpecError::UnknownName { step: 1, name }) if name == "x.lts"));
}

#[test]
fn rejects_self_reference() {
    let spec = spec_with_steps(r#"[["merc-lts", "$out.x.lts", "$x.lts"]]"#);
    assert!(matches!(resolve(&spec), Err(JobSpecError::UnknownName { step: 1, name }) if name == "x.lts"));
}

#[test]
fn rejects_malformed_reference() {
    let spec = spec_with_steps(r#"[["merc-lts", "$-oops"]]"#);
    assert!(matches!(resolve(&spec), Err(JobSpecError::MalformedReference { step: 1, arg }) if arg == "$-oops"));
}

#[test]
fn rejects_unknown_tool() {
    let spec = spec_with_steps(r#"[["rm", "-rf", "/"]]"#);
    assert!(matches!(resolve(&spec), Err(JobSpecError::UnknownTool { step: 1, tool }) if tool == "rm"));
}

#[test]
fn rejects_empty_step() {
    let spec = spec_with_steps(r#"[[]]"#);
    assert!(matches!(resolve(&spec), Err(JobSpecError::EmptyStep { step: 1 })));
}

#[test]
fn rejects_reserved_input_name() {
    let json = r#"{ "schema_version": 2, "inputs": { "out.x": "x" }, "steps": [["merc-lts"]] }"#;
    let spec = JobSpec::from_json(json).expect("valid spec");
    assert!(matches!(resolve(&spec), Err(JobSpecError::InvalidInputName(name)) if name == "out.x"));
}

#[test]
fn rejects_keeping_an_input() {
    let json = r#"{ "schema_version": 2, "inputs": { "spec": "x" }, "steps": [["merc-lts"]], "keep": ["$spec"] }"#;
    let spec = JobSpec::from_json(json).expect("valid spec");
    assert!(matches!(resolve(&spec), Err(JobSpecError::UnknownKeep(name)) if name == "spec"));
}

#[test]
fn rejects_malformed_keep() {
    let json = r#"{ "schema_version": 2, "steps": [["merc-lts", "$out.a"]], "keep": ["a"] }"#;
    let spec = JobSpec::from_json(json).expect("valid spec");
    assert!(matches!(resolve(&spec), Err(JobSpecError::MalformedKeep(entry)) if entry == "a"));
}

#[test]
fn rejects_unsupported_schema_version() {
    let json = r#"{ "schema_version": 1, "inputs": {}, "steps": [] }"#;
    let spec = JobSpec::from_json(json).expect("valid spec");
    assert!(matches!(resolve(&spec), Err(JobSpecError::UnsupportedSchemaVersion { found: 1, supported: 2 })));
}

#[test]
fn rejects_empty_pipeline() {
    let json = r#"{ "schema_version": 2, "inputs": {}, "steps": [] }"#;
    let spec = JobSpec::from_json(json).expect("valid spec");
    assert!(matches!(resolve(&spec), Err(JobSpecError::EmptyPipeline)));
}

#[test]
fn records_which_earlier_steps_a_step_depends_on() {
    let spec = spec_with_steps(
        r#"[
            ["merc-lts", "$spec", "$out.a.lts"],
            ["merc-lts", "$spec", "$out.b.lts"],
            ["merc-lts", "$b.lts", "$a.lts", "$b.lts", "$out.c.lts"],
            ["merc-lts", "$a.lts", "$out.d.lts"]
        ]"#,
    );
    let plan = resolve(&spec).expect("valid pipeline");

    let depends_on: Vec<_> = plan.steps.iter().map(|step| step.depends_on.clone()).collect();
    assert_eq!(depends_on, vec![vec![], vec![], vec![0, 1], vec![0]]);
}
