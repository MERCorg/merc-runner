use merc_core::Composition;
use merc_core::CompositionError;
use merc_core::Tool;
use merc_core::resolve;

/// Two parts composed once, and the result composed with a third part.
const COMPOSITION_JSON: &str = r#"{
  "default_reduction": "dpbranching-bisim",
  "machine_parts": [
    { "name": "A", "mcrl2_file": "models/A.mcrl2", "lps_file": "A.lps", "lts_file": "A.lts" },
    { "name": "B", "mcrl2_file": "B.mcrl2", "lps_file": "B.lps", "lts_file": "B.lts" },
    { "name": "C", "mcrl2_file": "C.mcrl2", "lps_file": "C.lps", "lts_file": "C.lts" }
  ],
  "composition_steps": [
    {
      "left_name": "A", "right_name": "B",
      "left_lts_file": "A.dpbranching-bisim.lts", "right_lts_file": "B.dpbranching-bisim.lts",
      "output_lts_file": "step1.lts",
      "comm": ["x_r|x_s -> x", "y_r|y_s -> y"],
      "allow": ["x|z", "y"],
      "hide": []
    },
    {
      "left_name": "R2", "right_name": "C",
      "left_lts_file": "step1.dpbranching-bisim.lts", "right_lts_file": "C.lts",
      "output_lts_file": "step2.lts",
      "allow": ["x"],
      "hide": ["x"]
    }
  ]
}"#;

fn argv(tool: Tool, args: &[String]) -> String {
    format!("{} {}", tool.binary_name(), args.join(" "))
}

#[test]
fn converts_a_composition_into_a_pipeline() {
    let spec = Composition::from_json(COMPOSITION_JSON).unwrap().to_job_spec().unwrap();
    assert_eq!(spec.inputs["A.mcrl2"], "models/A.mcrl2");

    let plan = resolve(&spec).expect("valid pipeline");
    let commands: Vec<_> = plan.steps.iter().map(|step| argv(step.tool, &step.args)).collect();
    assert_eq!(
        commands,
        vec![
            "mcrl22lps models/A.mcrl2 A.lps",
            "merc-lps explore-explicit A.lps --output A.lts",
            "mcrl22lps B.mcrl2 B.lps",
            "merc-lps explore-explicit B.lps --output B.lts",
            "mcrl22lps C.mcrl2 C.lps",
            "merc-lps explore-explicit C.lps --output C.lts",
            "merc-lts reduce dp-branching-bisim A.lts --output A.dpbranching-bisim.lts",
            "merc-lts reduce dp-branching-bisim B.lts --output B.dpbranching-bisim.lts",
            "merc-lts combine A.dpbranching-bisim.lts B.dpbranching-bisim.lts --output step1.lts --comm {x_r|x_s -> x, y_r|y_s -> y} --allow {x|z, y}",
            "merc-lts reduce dp-branching-bisim step1.lts --output step1.dpbranching-bisim.lts",
            "merc-lts combine step1.dpbranching-bisim.lts C.lts --output step2.lts --allow {x} --hide {x}",
        ]
    );
    assert_eq!(plan.keep, vec!["step2.lts"]);
}

#[test]
fn escapes_dollars_in_action_sets() {
    let json = COMPOSITION_JSON.replace(r#""allow": ["x"]"#, r#""allow": ["x$"]"#);
    let spec = Composition::from_json(&json).unwrap().to_job_spec().unwrap();
    let plan = resolve(&spec).expect("valid pipeline");
    assert!(plan.steps.last().unwrap().args.contains(&"{x$}".to_string()));
}

#[test]
fn rejects_an_operand_that_is_never_produced() {
    let json = COMPOSITION_JSON.replace(r#""right_lts_file": "C.lts""#, r#""right_lts_file": "D.lts""#);
    let err = Composition::from_json(&json).unwrap().to_job_spec().unwrap_err();
    assert!(matches!(err, CompositionError::UnknownLts { step: 2, file } if file == "D.lts"));
}

#[test]
fn rejects_a_file_produced_twice() {
    let json = COMPOSITION_JSON.replace(r#""step2.lts""#, r#""step1.lts""#);
    let err = Composition::from_json(&json).unwrap().to_job_spec().unwrap_err();
    assert!(matches!(err, CompositionError::DuplicateFile(file) if file == "step1.lts"));
}

#[test]
fn rejects_file_names_that_are_not_plain_names() {
    let json = COMPOSITION_JSON.replace(r#""A.lps""#, r#""out/A.lps""#);
    let err = Composition::from_json(&json).unwrap().to_job_spec().unwrap_err();
    assert!(matches!(err, CompositionError::InvalidFileName(file) if file == "out/A.lps"));
}
