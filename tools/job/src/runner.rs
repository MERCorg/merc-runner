//! Runs the steps of a plan as subprocesses, in parallel as far as the
//! [`Scheduler`] allows.
//!
//! Every step's stdout and stderr are merged into one pipe, so its output is
//! read in exactly the order the tool wrote it. A reader thread per running
//! step forwards each line to our stderr, prefixed with the step number and
//! tool, and writes it unchanged to the step's log file. Once the output ends
//! the thread reports the step's exit status to the main thread, which then
//! starts whatever steps have become ready.

use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::io;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

use duct::ReaderHandle;
use merc_core::ResolvedPlan;
use merc_core::Scheduler;

use crate::JobError;
use crate::ToolPaths;

/// A step that has been started.
struct Running {
    handle: Arc<ReaderHandle>,
    log: PathBuf,
}

/// Runs every step of `plan` in `workdir`, starting each as soon as the steps
/// it depends on have finished and enough of the `threads` are free. The
/// output of every step is logged to a file in `log_dir`.
///
/// If a step fails, the steps still running are killed and no new ones
/// start; this returns once all of them have exited and their logs are
/// complete.
pub fn run_plan(plan: &ResolvedPlan, tool_paths: &ToolPaths, workdir: &Path, log_dir: &Path, threads: NonZeroUsize) -> Result<(), JobError> {
    fs::create_dir_all(log_dir).map_err(|source| JobError::LogDir { path: log_dir.to_path_buf(), source })?;

    // Pad the step numbers in the prefixes, so the output of the steps lines up.
    let width = plan.steps.len().to_string().len();
    let (sender, receiver) = mpsc::channel::<(usize, Result<ExitStatus, JobError>)>();

    thread::scope(|scope| {
        let mut scheduler = Scheduler::new(plan, threads);
        let mut running: HashMap<usize, Running> = HashMap::new();
        let mut failure: Option<JobError> = None;

        loop {
            if failure.is_none() {
                for launch in scheduler.launch() {
                    let index = launch.step;
                    let number = index + 1;
                    let step = &plan.steps[index];
                    let tool = step.tool.binary_name();
                    let program = tool_paths.program(step.tool);
                    let args = step.command_args(launch.threads);
                    let log = log_dir.join(format!("{number}-{tool}.log"));

                    let plural = if launch.threads == 1 { "" } else { "s" };
                    eprintln!("==> [{number}] ({} thread{plural}) {} {}", launch.threads, program.display(), args.join(" "));

                    let started = File::create(&log)
                        .map_err(|source| JobError::CreateLog { path: log.clone(), source })
                        .and_then(|file| {
                            // An `OsString` rather than a path, which duct would make
                            // relative to the working directory instead of looking it up
                            // on `PATH`.
                            duct::cmd(program.clone().into_os_string(), &args)
                                .dir(workdir)
                                .stdin_null()
                                .stderr_to_stdout()
                                .unchecked()
                                .reader()
                                .map(|handle| (Arc::new(handle), file))
                                .map_err(|source| JobError::Spawn { step: number, program, source })
                        });

                    let (handle, file) = match started {
                        Ok(started) => started,
                        Err(err) => {
                            failure = Some(err);
                            kill_all(&running);
                            break;
                        }
                    };

                    let prefix = format!("[{number:>width$} {tool}] ");
                    let reader = Arc::clone(&handle);
                    let sender = sender.clone();
                    let log_path = log.clone();
                    scope.spawn(move || {
                        let result = forward(number, &reader, &prefix, file, &log_path);
                        // The main thread only stops receiving once every running
                        // step has reported, so this cannot fail.
                        let _ = sender.send((index, result));
                    });

                    running.insert(index, Running { handle, log });
                }
            }

            if running.is_empty() {
                // Without a failure the scheduler always has a step to start
                // while there are unfinished steps and nothing runs.
                debug_assert!(failure.is_some() || scheduler.is_done());
                break;
            }

            let (index, result) = receiver.recv().expect("every running step reports back");
            let step = running.remove(&index).expect("only running steps report back");
            let number = index + 1;

            let error = match result {
                Ok(status) if status.success() => None,
                Ok(status) => Some(JobError::StepFailed {
                    step: number,
                    tool: plan.steps[index].tool.binary_name(),
                    status,
                    log: step.log,
                }),
                Err(err) => {
                    // Reading the output failed, so the tool may still be running.
                    let _ = step.handle.kill();
                    Some(err)
                }
            };

            match error {
                // Steps killed after a failure are not reported.
                _ if failure.is_some() => {}
                None => {
                    eprintln!("<== [{number}] done");
                    scheduler.finish(index);
                }
                Some(err) => {
                    failure = Some(err);
                    kill_all(&running);
                }
            }
        }

        failure.map_or(Ok(()), Err)
    })
}

/// Kills every running step. Their reader threads then see the end of their
/// output and report back as usual.
fn kill_all(running: &HashMap<usize, Running>) {
    for step in running.values() {
        // The step may have exited in the meantime, which is fine.
        let _ = step.handle.kill();
    }
}

/// Forwards the output of step `number` line by line to our stderr, prefixed
/// with `prefix`, and unchanged to `log`, until the output ends. Then returns
/// the step's exit status.
fn forward(number: usize, handle: &ReaderHandle, prefix: &str, log: File, log_path: &Path) -> Result<ExitStatus, JobError> {
    let write_error = |source| JobError::WriteLog { path: log_path.to_path_buf(), source };

    let mut reader = BufReader::new(handle);
    let mut log = BufWriter::new(log);
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|source| JobError::ReadOutput { step: number, source })?;
        if read == 0 {
            break;
        }

        log.write_all(&line).map_err(write_error)?;

        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\n', '\r']);
        // Written as a single line through the lock, so lines of steps running
        // at the same time are never mixed up. A console that went away is no
        // reason to stop the step, so errors are ignored.
        let _ = writeln!(io::stderr().lock(), "{prefix}{text}");
    }
    log.flush().map_err(write_error)?;

    // Reaching the end of the output waits for the tool to exit.
    let output = handle
        .try_wait()
        .map_err(|source| JobError::ReadOutput { step: number, source })?
        .expect("the step has exited once its output has ended");
    Ok(output.status)
}
