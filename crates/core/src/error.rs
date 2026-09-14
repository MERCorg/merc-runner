use thiserror::Error;

#[derive(Debug, Error)]
pub enum JobSpecError {
    #[error("failed to parse job spec: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("unsupported schema_version {found} (this crate implements {supported})")]
    UnsupportedSchemaVersion { found: u32, supported: u32 },

    #[error("job spec has no steps")]
    EmptyPipeline,

    #[error("step {step} is empty; expected at least a tool name")]
    EmptyStep { step: usize },

    #[error("step {step} runs unknown tool `{tool}`")]
    UnknownTool { step: usize, tool: String },

    #[error("invalid input name `{0}`")]
    InvalidInputName(String),

    #[error("`{name}` is declared twice (step {step})")]
    DuplicateName { step: usize, name: String },

    #[error("step {step} references `${name}`, which is neither an input nor an output of an earlier step")]
    UnknownName { step: usize, name: String },

    #[error("step {step} has a malformed reference in argument `{arg}`")]
    MalformedReference { step: usize, arg: String },

    #[error("`keep` entry `{0}` must be a single `$<name>` reference")]
    MalformedKeep(String),

    #[error("`keep` entry `${0}` is not an output of any step")]
    UnknownKeep(String),
}
