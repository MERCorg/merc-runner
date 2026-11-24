use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;

use crate::error::JobSpecError;

/// The schema version this crate supports.
pub const SCHEMA_VERSION: u32 = 2;

/// A full pipeline job spec, as uploaded to S3 and consumed by the runner.
///
/// ```json
/// {
///   "schema_version": 2,
///   "inputs": { "spec": "s3://bucket/abp.mcrl2" },
///   "steps": [
///     ["mcrl22lps", "-v", "$spec", "$out.model.lps"],
///     ["merc-lps", "explore-explicit", "$model.lps", "--output", "$out.model.lts"]
///   ],
///   "keep": ["$model.lts"]
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobSpec {
    pub schema_version: u32,

    /// Named inputs, mapped to the location (an `s3://` URI or a local path)
    /// they are fetched from. Referenced by steps as `$<name>`.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,

    /// Pipeline steps, executed in array order. Each step is the tool's argv:
    /// the first element names the [`Tool`], the rest are its arguments
    /// (including any subcommand).
    pub steps: Vec<Vec<String>>,

    /// Outputs to keep after the job finishes, as `$<name>` references.
    /// Everything else is an intermediate and may be discarded.
    #[serde(default)]
    pub keep: Vec<String>,
}

impl JobSpec {
    /// Parses a job spec from its canonical JSON form. Does not validate
    /// references or tool names — use [`crate::resolve::resolve`] for that.
    pub fn from_json(json: &str) -> Result<Self, JobSpecError> {
        Ok(serde_json::from_str(json)?)
    }
}

/// The toolset a [`Tool`] belongs to, which determines where its binary is
/// installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toolset {
    /// The mCRL2 toolset (`mcrl22lps`, `lps2pbes`, ...).
    Mcrl2,
    /// The merc toolset (`merc-lps`, `merc-lts`, ...).
    Merc,
}

/// The allow-listed set of binaries the runner may execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tool {
    #[serde(rename = "mcrl22lps")]
    Mcrl22Lps,
    #[serde(rename = "lps2pbes")]
    Lps2Pbes,
    #[serde(rename = "merc-lps")]
    MercLps,
    #[serde(rename = "merc-lts")]
    MercLts,
    #[serde(rename = "merc-pbes")]
    MercPbes,
    #[serde(rename = "merc-vpg")]
    MercVpg,
    #[serde(rename = "merc-rewrite")]
    MercRewrite,
}

impl Tool {
    const ALL: [Tool; 7] = [
        Tool::Mcrl22Lps,
        Tool::Lps2Pbes,
        Tool::MercLps,
        Tool::MercLts,
        Tool::MercPbes,
        Tool::MercVpg,
        Tool::MercRewrite,
    ];

    /// The binary name to invoke for this tool, matching its `PATH` entry on
    /// the runner AMI.
    pub fn binary_name(self) -> &'static str {
        match self {
            Tool::Mcrl22Lps => "mcrl22lps",
            Tool::Lps2Pbes => "lps2pbes",
            Tool::MercLps => "merc-lps",
            Tool::MercLts => "merc-lts",
            Tool::MercPbes => "merc-pbes",
            Tool::MercVpg => "merc-vpg",
            Tool::MercRewrite => "merc-rewrite",
        }
    }

    /// The toolset this tool is part of.
    pub fn toolset(self) -> Toolset {
        match self {
            Tool::Mcrl22Lps | Tool::Lps2Pbes => Toolset::Mcrl2,
            Tool::MercLps | Tool::MercLts | Tool::MercPbes | Tool::MercVpg | Tool::MercRewrite => Toolset::Merc,
        }
    }

    /// Looks up the tool whose binary name is `name`.
    pub fn from_binary_name(name: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|tool| tool.binary_name() == name)
    }
}
