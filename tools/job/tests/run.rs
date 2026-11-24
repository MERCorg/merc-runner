//! Runs `merc-job` on specs whose tools are fake shell scripts: every tool
//! runs its first argument as a shell snippet, so a step like
//! `["merc-lts", "echo hi; exit 3", "$out.a"]` prints `hi` and fails. The
//! remaining arguments only declare outputs and references, which determine
//! the dependencies between the steps.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;
use std::time::Duration;
use std::time::Instant;

/// The tools a spec can use, each a fake.
const TOOLS: [&str; 3] = ["mcrl22lps", "merc-lps", "merc-lts"];

const FAKE_TOOL: &str = r#"#!/bin/sh
PATH="$(dirname "$0"):$PATH"
exec /bin/sh -c "$1"
"#;

/// `rendezvous <self> <other> <tries>` announces that `self` has started and
/// waits for `other` to do the same, failing after `tries` checks 50ms apart.
/// Two steps can only both succeed if they run at the same time.
const RENDEZVOUS: &str = r#"#!/bin/sh
touch "$1.started"
i=0
while [ ! -e "$2.started" ]; do
    i=$((i + 1))
    [ "$i" -gt "$3" ] && exit 1
    sleep 0.05
done
"#;

/// A fresh directory for the test `name`, with the fake tools in `bin/`.
fn setup(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("bin")).unwrap();
    fs::create_dir_all(dir.join("work")).unwrap();

    let script = |name: &str, content: &str| {
        let path = dir.join("bin").join(name);
        fs::write(&path, content).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    for tool in TOOLS {
        script(tool, FAKE_TOOL);
    }
    script("rendezvous", RENDEZVOUS);

    dir
}

/// Runs the spec with the given `steps` (a JSON array) in `dir`.
fn run(dir: &Path, steps: &str, args: &[&str]) -> (Output, String) {
    let spec = dir.join("spec.json");
    fs::write(&spec, format!(r#"{{ "schema_version": 2, "steps": {steps} }}"#)).unwrap();

    let bin = dir.join("bin");
    let output = Command::new(env!("CARGO_BIN_EXE_merc-job"))
        .arg("--merc-path")
        .arg(&bin)
        .arg("--mcrl2-path")
        .arg(&bin)
        .arg("--workdir")
        .arg(dir.join("work"))
        .args(args)
        .arg(&spec)
        .output()
        .unwrap();

    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    (output, stderr)
}

fn log(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join("work/logs").join(name)).unwrap()
}

const RENDEZVOUS_STEPS: &str = r#"[
    ["merc-lts", "rendezvous a b 20", "$out.a"],
    ["merc-lts", "rendezvous b a 20", "$out.b"],
    ["merc-lps", "echo joined", "$a", "$b"]
]"#;

#[test]
fn runs_independent_steps_in_parallel() {
    let dir = setup("parallel");
    let (output, stderr) = run(&dir, RENDEZVOUS_STEPS, &["--threads", "2"]);

    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("[3 merc-lps] joined"), "{stderr}");
}

#[test]
fn stays_within_the_thread_budget() {
    let dir = setup("budget");
    let (output, stderr) = run(&dir, RENDEZVOUS_STEPS, &["--threads", "1"]);

    // Step 1 waits in vain for step 2, which cannot start next to it.
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("step 1 (`merc-lts`) failed"), "{stderr}");
    assert!(!stderr.contains("==> [2]"), "{stderr}");
}

#[test]
fn logs_the_output_of_every_step() {
    let dir = setup("logs");
    let (output, stderr) = run(
        &dir,
        r#"[["merc-lts", "echo one; echo two >&2; echo three"], ["merc-lps", "echo other"]]"#,
        &[],
    );

    assert!(output.status.success(), "{stderr}");
    assert!(output.stdout.is_empty());

    // stdout and stderr are merged in the order they were written.
    assert_eq!(log(&dir, "1-merc-lts.log"), "one\ntwo\nthree\n");
    assert_eq!(log(&dir, "2-merc-lps.log"), "other\n");

    for line in ["[1 merc-lts] one", "[1 merc-lts] two", "[1 merc-lts] three", "[2 merc-lps] other"] {
        assert!(stderr.contains(line), "missing `{line}` in:\n{stderr}");
    }
}

#[test]
fn a_failing_step_kills_the_others() {
    let dir = setup("failure");
    let start = Instant::now();
    let (output, stderr) = run(
        &dir,
        // `exec`, so that killing the tool kills the sleep, rather than leaving
        // it behind holding the output pipe open.
        r#"[
            ["merc-lts", "exec sleep 30", "$out.a"],
            ["merc-lts", "echo failing; sleep 0.2; exit 3"],
            ["merc-lps", "echo never", "$a"]
        ]"#,
        &["--threads", "2"],
    );

    assert!(start.elapsed() < Duration::from_secs(10), "the sleeping step was not killed");
    assert!(!output.status.success());
    assert!(stderr.contains("step 2 (`merc-lts`) failed: exit status: 3"), "{stderr}");
    assert!(stderr.contains("2-merc-lts.log"), "the error names the log:\n{stderr}");
    assert!(!stderr.contains("==> [3]"), "{stderr}");
    assert_eq!(log(&dir, "2-merc-lts.log"), "failing\n");
}

#[test]
fn reports_a_tool_that_cannot_be_started() {
    let dir = setup("missing");
    fs::remove_file(dir.join("bin/merc-lps")).unwrap();
    let (output, stderr) = run(&dir, r#"[["merc-lps", "true"]]"#, &[]);

    assert!(!output.status.success());
    assert!(stderr.contains("failed to run"), "{stderr}");
}
