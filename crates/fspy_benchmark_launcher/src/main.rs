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

/// With `--relative`, the target opens this bare name from the filesystem
/// root instead, driving the tracker's relative-path lane: it must resolve
/// the working directory and join the two. Root as the working directory
/// makes the joined result exactly [`MISSING_PATH`], so validation checks
/// the same captured path in both modes.
const MISSING_RELATIVE_PATH: &str = ".fspy-benchmark-missing";
#[cfg(unix)]
const ROOT_DIR: &str = "/";
#[cfg(windows)]
const ROOT_DIR: &str = r"C:\";

fn main() {
    let mut args = env::args_os().skip(1).collect::<Vec<_>>();
    let mut mode = Mode::Tracked;
    let mut relative = false;
    while let Some(flag) = args.first().map(OsString::as_os_str) {
        if flag == "--untracked" {
            mode = Mode::Untracked;
        } else if flag == "--validate" {
            mode = Mode::Validate;
        } else if flag == "--relative" {
            relative = true;
        } else {
            break;
        }
        args.remove(0);
    }
    let (target, target_args) =
        args.split_first().expect("usage: fspy_benchmark_launcher [FLAGS] TARGET ARGS...");

    let runtime = Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        match mode {
            Mode::Tracked => report(run_tracked(target, target_args, relative).await).await,
            Mode::Untracked => report(run_untracked(target, target_args, relative).await).await,
            Mode::Validate => validate(target, target_args, relative).await,
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
async fn run_tracked(target: &OsString, target_args: &[OsString], relative: bool) -> Launch {
    let mut command = Command::new(target);
    command
        .args(target_args)
        .arg(if relative { MISSING_RELATIVE_PATH } else { MISSING_PATH })
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if relative {
        command.current_dir(ROOT_DIR);
    }
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

async fn run_untracked(target: &OsString, target_args: &[OsString], relative: bool) -> Launch {
    let mut command = tokio::process::Command::new(target);
    command
        .args(target_args)
        .arg(if relative { MISSING_RELATIVE_PATH } else { MISSING_PATH })
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if relative {
        command.current_dir(ROOT_DIR);
    }
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
/// In relative mode the captured path must come out identical — the tracker
/// resolves the root working directory and joins the bare name back into
/// [`MISSING_PATH`] — so the assertion below covers both modes.
async fn validate(target: &OsString, target_args: &[OsString], relative: bool) {
    let mut command = Command::new(target);
    command
        .args(target_args)
        .arg(if relative { MISSING_RELATIVE_PATH } else { MISSING_PATH })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    if relative {
        command.current_dir(ROOT_DIR);
    }
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
