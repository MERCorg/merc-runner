//! Reader for `composition.json` files, which describe a compositional
//! state-space generation: every machine part is linearised, explored and
//! reduced on its own, and the results are then combined pairwise, reducing
//! after each step.
//!
//! [`Composition::to_job_spec`] turns such a file into an ordinary
//! [`JobSpec`], which is then resolved and executed like any other spec.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

use crate::resolve::is_valid_input_name;
use crate::resolve::is_valid_name;
use crate::spec::JobSpec;
use crate::spec::SCHEMA_VERSION;
use crate::spec::Tool;

/// A compositional verification job, as read from `composition.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Composition {
    /// The equivalence every intermediate LTS is reduced modulo, using the
    /// mCRL2 name (e.g. `dpbranching-bisim`). The reduced LTS of `x.lts` is
    /// expected to be named `x.<default_reduction>.lts`.
    pub default_reduction: String,

    /// The components of the system, each generated separately.
    pub machine_parts: Vec<MachinePart>,

    /// The pairwise compositions, executed in order.
    pub composition_steps: Vec<CompositionStep>,
}

/// One component of the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachinePart {
    pub name: String,

    /// The mCRL2 specification of this part, relative to the composition file.
    pub mcrl2_file: String,

    /// The file the linearised specification is written to.
    pub lps_file: String,

    /// The file the explored state space is written to.
    pub lts_file: String,
}

/// The parallel composition `hide(allow(comm(left || right)))`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompositionStep {
    pub left_name: String,
    pub right_name: String,

    /// The left operand: an LTS of a machine part or of an earlier step, or
    /// the reduced version of one.
    pub left_lts_file: String,

    /// The right operand, like [`Self::left_lts_file`].
    pub right_lts_file: String,

    /// The file the composition is written to.
    pub output_lts_file: String,

    /// Communication expressions, e.g. `a_r|a_s -> a`.
    #[serde(default)]
    pub comm: Vec<String>,

    /// Allowed multi-actions, e.g. `a|b`.
    #[serde(default)]
    pub allow: Vec<String>,

    /// Actions to hide.
    #[serde(default)]
    pub hide: Vec<String>,
}

#[derive(Debug, Error)]
pub enum CompositionError {
    #[error("failed to parse composition: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("composition has no machine parts")]
    NoMachineParts,

    #[error("machine part `{part}` has mcrl2_file `{file}`, which has no file name")]
    MissingFileName { part: String, file: String },

    #[error("`{0}` is not a valid file name; use only letters, digits, `_`, `.` and `-`")]
    InvalidFileName(String),

    #[error("`{0}` is produced more than once")]
    DuplicateFile(String),

    #[error("composition step {step} uses `{file}`, which is not produced by a machine part or an earlier step")]
    UnknownLts { step: usize, file: String },
}

impl Composition {
    /// Parses a composition from its JSON form.
    pub fn from_json(json: &str) -> Result<Self, CompositionError> {
        Ok(serde_json::from_str(json)?)
    }

    /// Converts the composition into a job spec that runs, in order:
    ///
    /// 1. per machine part: `mcrl22lps` and `merc-lps explore-explicit`;
    /// 2. per composition step: `merc-lts combine`, preceded by a
    ///    `merc-lts reduce` for each operand that names a reduced LTS
    ///    (`x.<default_reduction>.lts` for a produced `x.lts`).
    ///
    /// The output of the last composition step is kept, or every part's LTS
    /// if there are no composition steps.
    pub fn to_job_spec(&self) -> Result<JobSpec, CompositionError> {
        if self.machine_parts.is_empty() {
            return Err(CompositionError::NoMachineParts);
        }

        let mut builder = Builder::new(&self.default_reduction);
        let mut inputs = BTreeMap::new();

        for part in &self.machine_parts {
            let input = Path::new(&part.mcrl2_file)
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| CompositionError::MissingFileName {
                    part: part.name.clone(),
                    file: part.mcrl2_file.clone(),
                })?;
            if !is_valid_input_name(input) {
                return Err(CompositionError::InvalidFileName(input.to_string()));
            }
            builder.declare(input)?;
            inputs.insert(input.to_string(), part.mcrl2_file.clone());

            builder.declare(&part.lps_file)?;
            builder.step(vec![
                Tool::Mcrl22Lps.binary_name().to_string(),
                reference(input),
                declare(&part.lps_file),
            ]);

            builder.declare_lts(&part.lts_file)?;
            builder.step(vec![
                Tool::MercLps.binary_name().to_string(),
                "explore-explicit".to_string(),
                reference(&part.lps_file),
                "--output".to_string(),
                declare(&part.lts_file),
            ]);
        }

        for (index, step) in self.composition_steps.iter().enumerate() {
            builder.require(index + 1, &step.left_lts_file)?;
            builder.require(index + 1, &step.right_lts_file)?;

            builder.declare_lts(&step.output_lts_file)?;
            let mut argv = vec![
                Tool::MercLts.binary_name().to_string(),
                "combine".to_string(),
                reference(&step.left_lts_file),
                reference(&step.right_lts_file),
                "--output".to_string(),
                declare(&step.output_lts_file),
            ];
            for (flag, items) in [("--comm", &step.comm), ("--allow", &step.allow), ("--hide", &step.hide)] {
                // An empty set means the same as leaving the operator out.
                if !items.is_empty() {
                    argv.push(flag.to_string());
                    argv.push(literal(&format!("{{{}}}", items.join(", "))));
                }
            }
            builder.step(argv);
        }

        let keep = match self.composition_steps.last() {
            Some(last) => vec![reference(&last.output_lts_file)],
            None => self.machine_parts.iter().map(|part| reference(&part.lts_file)).collect(),
        };

        Ok(JobSpec {
            schema_version: SCHEMA_VERSION,
            inputs,
            steps: builder.steps,
            keep,
        })
    }
}

/// Accumulates the steps of the generated spec and tracks which files exist.
struct Builder<'a> {
    reduction: &'a str,
    steps: Vec<Vec<String>>,

    /// Every name declared so far.
    declared: HashSet<String>,

    /// Not-yet-produced reduced LTS names, mapped to the LTS they reduce.
    reducible: HashMap<String, String>,
}

impl<'a> Builder<'a> {
    fn new(reduction: &'a str) -> Self {
        Self {
            reduction,
            steps: Vec::new(),
            declared: HashSet::new(),
            reducible: HashMap::new(),
        }
    }

    fn step(&mut self, argv: Vec<String>) {
        self.steps.push(argv);
    }

    /// Declares the file `name`, which must be new.
    fn declare(&mut self, name: &str) -> Result<(), CompositionError> {
        if !is_valid_name(name) {
            return Err(CompositionError::InvalidFileName(name.to_string()));
        }
        if !self.declared.insert(name.to_string()) {
            return Err(CompositionError::DuplicateFile(name.to_string()));
        }
        Ok(())
    }

    /// Declares the LTS `name`, and makes its reduced version available to
    /// later composition steps.
    fn declare_lts(&mut self, name: &str) -> Result<(), CompositionError> {
        self.declare(name)?;

        let stem = name.strip_suffix(".lts").unwrap_or(name);
        self.reducible.insert(format!("{stem}.{}.lts", self.reduction), name.to_string());
        Ok(())
    }

    /// Ensures `file` exists before composition step `step`, emitting the
    /// reduction that produces it if needed.
    fn require(&mut self, step: usize, file: &str) -> Result<(), CompositionError> {
        if self.declared.contains(file) {
            return Ok(());
        }

        let source = self.reducible.remove(file).ok_or_else(|| CompositionError::UnknownLts {
            step,
            file: file.to_string(),
        })?;

        self.declare(file)?;
        let equivalence = merc_equivalence(self.reduction);
        self.step(vec![
            Tool::MercLts.binary_name().to_string(),
            "reduce".to_string(),
            literal(equivalence),
            reference(&source),
            "--output".to_string(),
            declare(file),
        ]);
        Ok(())
    }
}

/// Maps an mCRL2 (`ltsconvert`) equivalence name to the `merc-lts reduce`
/// name. Unknown names are passed through, so merc names work as well.
fn merc_equivalence(mcrl2: &str) -> &str {
    match mcrl2 {
        "bisim" => "strong-bisim",
        "dpbranching-bisim" => "dp-branching-bisim",
        "dpweak-bisim" => "dp-weak-bisim",
        other => other,
    }
}

/// An argument that references `name`.
fn reference(name: &str) -> String {
    format!("${name}")
}

/// An argument that declares the output `name`.
fn declare(name: &str) -> String {
    format!("$out.{name}")
}

/// An argument that is passed through literally.
fn literal(text: &str) -> String {
    text.replace('$', "$$")
}
