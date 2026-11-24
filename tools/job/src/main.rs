//! Executes a merc-cloud job spec: a JSON file whose steps are resolved by
//! `merc_core::resolve` and then run as subprocesses, with independent steps
//! running in parallel as scheduled by `merc_core::Scheduler`. See
//! [`runner`] for how steps are run and their output is logged.

mod runner;

use std::fs;
use std::io;
use std::num::NonZeroUsize;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::process::ExitStatus;
use std::thread;

use clap::Parser;
use merc_core::Composition;
use merc_core::CompositionError;
use merc_core::JobSpec;
use merc_core::JobSpecError;
use merc_core::ResolvedPlan;
use merc_core::Tool;
use merc_core::Toolset;
use merc_core::resolve;
use thiserror::Error;

/// Executes a merc-cloud job spec using the merc toolset.
#[derive(Debug, Parser)]
struct Cli {
    /// Path to the job spec JSON file.
    spec: PathBuf,

    /// Read the file as a `composition.json` instead of a job spec, and run
    /// the job spec it converts to.
    #[arg(long)]
    composition: bool,

    /// Directory steps run in and write their outputs to. Defaults to the
    /// current directory.
    #[arg(long)]
    workdir: Option<PathBuf>,

    /// Directory the output of every step is logged to, as
    /// `<step>-<tool>.log`. Defaults to `logs` inside the working directory.
    #[arg(long)]
    log_dir: Option<PathBuf>,

    /// Validate the spec and print the commands that would run, without
    /// running them.
    #[arg(long)]
    dry_run: bool,

    /// The maximum number of threads used by all running steps together.
    /// Independent steps run in parallel as long as they fit. Defaults to the
    /// number of available CPUs.
    #[arg(long)]
    threads: Option<NonZeroUsize>,

    /// Directory containing the merc tool binaries (`merc-lps`, ...).
    /// Defaults to looking them up on `PATH`.
    #[arg(long)]
    merc_path: Option<PathBuf>,

    /// Directory containing the mCRL2 tool binaries (`mcrl22lps`, ...).
    /// Defaults to looking them up on `PATH`.
    #[arg(long)]
    mcrl2_path: Option<PathBuf>,
}

/// Where to find the binaries of each toolset.
struct ToolPaths {
    merc: Option<PathBuf>,
    mcrl2: Option<PathBuf>,
}

impl ToolPaths {
    /// Makes the given directories absolute, so they still resolve when a
    /// step runs in a different working directory.
    fn new(merc: Option<&Path>, mcrl2: Option<&Path>) -> Result<Self, JobError> {
        let absolute = |dir: Option<&Path>| {
            dir.map(|dir| std::path::absolute(dir).map_err(|source| JobError::ToolPath { path: dir.to_path_buf(), source }))
                .transpose()
        };

        Ok(Self { merc: absolute(merc)?, mcrl2: absolute(mcrl2)? })
    }

    /// The program to execute for `tool`: either the binary inside the
    /// configured directory, or the bare name to look up on `PATH`.
    fn program(&self, tool: Tool) -> PathBuf {
        let dir = match tool.toolset() {
            Toolset::Merc => &self.merc,
            Toolset::Mcrl2 => &self.mcrl2,
        };

        let name = format!("{}{}", tool.binary_name(), std::env::consts::EXE_SUFFIX);
        match dir {
            Some(dir) => dir.join(name),
            None => PathBuf::from(name),
        }
    }
}

#[derive(Debug, Error)]
enum JobError {
    #[error("failed to read job spec at {path}: {source}")]
    ReadSpec { path: PathBuf, source: io::Error },

    #[error("invalid job spec: {0}")]
    Spec(#[from] JobSpecError),

    #[error("invalid composition: {0}")]
    Composition(#[from] CompositionError),

    #[error("input `{name}` is `{location}`, but only local inputs are supported when running locally")]
    RemoteInput { name: String, location: String },

    #[error("invalid tool directory {path}: {source}")]
    ToolPath { path: PathBuf, source: io::Error },

    #[error("failed to run `{}` for step {step}: {source}", program.display())]
    Spawn { step: usize, program: PathBuf, source: io::Error },

    #[error("failed to create log directory {}: {source}", path.display())]
    LogDir { path: PathBuf, source: io::Error },

    #[error("failed to create log {}: {source}", path.display())]
    CreateLog { path: PathBuf, source: io::Error },

    #[error("failed to read the output of step {step}: {source}")]
    ReadOutput { step: usize, source: io::Error },

    #[error("failed to write log {}: {source}", path.display())]
    WriteLog { path: PathBuf, source: io::Error },

    #[error("step {step} (`{tool}`) failed: {status}; see {}", log.display())]
    StepFailed { step: usize, tool: &'static str, status: ExitStatus, log: PathBuf },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if let Err(err) = run(&cli) {
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn run(cli: &Cli) -> Result<(), JobError> {
    let json = fs::read_to_string(&cli.spec).map_err(|source| JobError::ReadSpec { path: cli.spec.clone(), source })?;
    let mut spec = if cli.composition {
        Composition::from_json(&json)?.to_job_spec()?
    } else {
        JobSpec::from_json(&json)?
    };

    // Local inputs are relative to the spec file, so a spec and its models
    // can be moved around together.
    // Remote inputs are only rejected when actually running, so a dry run
    // can still validate a spec meant for the cloud.
    let base = cli.spec.parent().unwrap_or(Path::new("."));
    for (name, location) in &mut spec.inputs {
        if !location.contains("://") {
            *location = base.join(&*location).display().to_string();
        } else if !cli.dry_run {
            return Err(JobError::RemoteInput { name: name.clone(), location: location.clone() });
        }
    }

    let plan = resolve(&spec)?;
    let tool_paths = ToolPaths::new(cli.merc_path.as_deref(), cli.mcrl2_path.as_deref())?;

    if cli.dry_run {
        print_plan(&plan, &tool_paths);
        return Ok(());
    }

    let threads = cli
        .threads
        .unwrap_or_else(|| thread::available_parallelism().unwrap_or(NonZeroUsize::MIN));
    let workdir = cli.workdir.as_deref().unwrap_or(Path::new("."));
    let log_dir = cli.log_dir.clone().unwrap_or_else(|| workdir.join("logs"));
    runner::run_plan(&plan, &tool_paths, workdir, &log_dir, threads)?;

    eprintln!("logs: {}", log_dir.display());
    for output in &plan.keep {
        eprintln!("kept: {}", workdir.join(output).display());
    }

    Ok(())
}

fn print_plan(plan: &ResolvedPlan, tool_paths: &ToolPaths) {
    for (index, step) in plan.steps.iter().enumerate() {
        print!("[{}] {} {}", index + 1, tool_paths.program(step.tool).display(), step.args.join(" "));
        if !step.depends_on.is_empty() {
            let after: Vec<String> = step.depends_on.iter().map(|dependency| (dependency + 1).to_string()).collect();
            print!("  (after {})", after.join(", "));
        }
        println!();
    }
    for output in &plan.keep {
        println!("keep {output}");
    }
}
