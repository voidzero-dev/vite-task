//! Launches the benchmark target once and reports what the launch cost.
//!
//! This is the only benchmark piece that links fspy. The harness compares two
//! builds of this binary — one against the fspy under review and one against
//! the baseline fspy — so both revisions must be measured by identical code:
//! the harness always builds this crate from the same source for both.
//!
//! Prints one line to stdout: the wall clock of the launch in nanoseconds,
//! followed by the batch timing the target reported for itself.

use std::{env, ffi::OsString, process::Stdio, time::Instant};

use fspy::Command;
use tokio::{io::AsyncReadExt as _, process::ChildStdout, runtime::Builder};
use tokio_util::sync::CancellationToken;

/// The base path the target opens, appended to its arguments so that only this
/// crate names it: the target derives its per-thread paths from it, and
/// validation asserts the bare path was captured.
#[cfg(unix)]
const MISSING_PATH: &str = "/.fspy-benchmark-missing";
#[cfg(windows)]
const MISSING_PATH: &str = r"C:\.fspy-benchmark-missing";

fn main() {
    let mut args = env::args_os().skip(1).collect::<Vec<_>>();
    let mode = match args.first().map(OsString::as_os_str) {
        Some(arg) if arg == "--untracked" => {
            args.remove(0);
            Mode::Untracked
        }
        Some(arg) if arg == "--validate" => {
            args.remove(0);
            Mode::Validate
        }
        _ => Mode::Tracked,
    };
    let (target, target_args) =
        args.split_first().expect("usage: fspy_benchmark_launcher [MODE] TARGET ARGS...");

    let runtime = Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        match mode {
            Mode::Tracked => report(run_tracked(target, target_args).await).await,
            Mode::Untracked => report(run_untracked(target, target_args).await).await,
            Mode::Validate => validate(target, target_args).await,
        }
    });
}

enum Mode {
    Tracked,
    Untracked,
    Validate,
}

struct Launch {
    wall_nanos: u128,
    stdout: ChildStdout,
}

/// Times the launch from just before the spawn to just after the wait, so
/// that a tracked launch covers session setup, injection, and teardown, and
/// nothing of this launcher's own startup.
async fn run_tracked(target: &OsString, target_args: &[OsString]) -> Launch {
    let mut command = Command::new(target);
    command
        .args(target_args)
        .arg(MISSING_PATH)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let start = Instant::now();
    let mut child = command
        .spawn(CancellationToken::new())
        .await
        .expect("failed to spawn tracked benchmark target");
    let stdout = child.stdout.take().expect("tracked benchmark target has no stdout");
    let termination = child.wait_handle.await.expect("failed to wait for tracked target");
    let wall_nanos = start.elapsed().as_nanos();
    assert!(
        termination.status.success(),
        "tracked benchmark target failed: {}",
        termination.status
    );
    Launch { wall_nanos, stdout }
}

async fn run_untracked(target: &OsString, target_args: &[OsString]) -> Launch {
    let mut command = tokio::process::Command::new(target);
    command
        .args(target_args)
        .arg(MISSING_PATH)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let start = Instant::now();
    let mut child = command.spawn().expect("failed to spawn untracked benchmark target");
    let stdout = child.stdout.take().expect("untracked benchmark target has no stdout");
    let status = child.wait().await.expect("failed to wait for untracked target");
    let wall_nanos = start.elapsed().as_nanos();
    assert!(status.success(), "untracked benchmark target failed: {status}");
    Launch { wall_nanos, stdout }
}

/// Relays the sample the target timed for itself after the wall clock, so the
/// harness reads every number from one line.
async fn report(mut launch: Launch) {
    let mut sample = Vec::new();
    launch.stdout.read_to_end(&mut sample).await.expect("failed to read the reported sample");
    let sample = str::from_utf8(&sample).expect("the reported sample is not utf-8");
    #[expect(clippy::print_stdout, reason = "the harness reads the measurement from stdout")]
    {
        println!("{} {}", launch.wall_nanos, sample.trim());
    }
}

/// Runs the target tracked and asserts that its accesses were captured, so
/// that the harness never benchmarks tracking that silently stopped working.
async fn validate(target: &OsString, target_args: &[OsString]) {
    let mut command = Command::new(target);
    command
        .args(target_args)
        .arg(MISSING_PATH)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let termination = command
        .spawn(CancellationToken::new())
        .await
        .expect("failed to spawn tracked benchmark target")
        .wait_handle
        .await
        .expect("failed to wait for tracked target");
    assert!(termination.status.success(), "benchmark target failed: {}", termination.status);
    let captured_missing_access = termination.path_accesses.iter().any(|access| {
        access.path.strip_path_prefix(MISSING_PATH, |result| {
            result.is_ok_and(|path| path.as_os_str().is_empty())
        })
    });
    assert!(captured_missing_access, "failed to capture access to {MISSING_PATH}");
}
