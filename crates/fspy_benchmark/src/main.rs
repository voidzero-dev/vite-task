//! Compares what two fspy builds cost the processes they track.
//!
//! Every earlier attempt at this benchmark compared a pull request run with a
//! baseline recorded by an earlier run on `main`, and kept fighting the same
//! fact: two runs land on two runner instances, and identically configured
//! instances disagree on every absolute and relative measurement by more than
//! a regression worth catching. This harness never compares across runs.
//! Instead the workflow builds the launcher twice — once against the fspy
//! under review and once against the baseline fspy — and this harness
//! interleaves launches of both builds on the same machine, in the same run,
//! as neighboring processes. Whatever the runner instance is doing to the
//! numbers, it is doing to both sides.
//!
//! Each iteration launches every arm — baseline-tracked, head-tracked, and
//! untracked — back to back in rotating order, and yields the ratio of head
//! to baseline per metric. The reported change is the median of those ratios,
//! so it prices exactly one thing: what the fspy change under review costs a
//! tracked process. The untracked arm prices tracking itself, as context.

use std::{
    env,
    ffi::{OsStr, OsString},
    process::{Command, Stdio},
};

const HEAD_LAUNCHER: &str = env!("CARGO_BIN_FILE_FSPY_BENCHMARK_LAUNCHER");
const DYNAMIC_TARGET: &str = env!("CARGO_BIN_FILE_FSPY_BENCHMARK_TARGET");
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const STATIC_TARGET: &str = env!("CARGO_BIN_FILE_FSPY_BENCHMARK_STATIC_TARGET");

/// Opens one small batch, only to prove that capturing works before measuring.
const VALIDATION_ARGS: [&str; 2] = ["1", "8"];

struct Workload {
    /// Thread count the target spawns, passed through to it.
    threads: &'static str,
    /// Opens per target thread, passed through to it.
    opens: &'static str,
    /// Measured iterations. Each one launches every arm once.
    iterations: usize,
    /// Unmeasured iterations run first, to fill caches and settle the runner.
    warmup: usize,
}

/// Launches cost several times more wall clock on Windows, so it affords
/// fewer of them in the same time.
#[cfg(windows)]
const LAUNCH_ITERATIONS: usize = 150;
#[cfg(not(windows))]
const LAUNCH_ITERATIONS: usize = 300;

/// Opens nothing, so the whole launch is the cost of starting a tracked
/// process. Two threads, because a process that fspy tracks rarely has one.
const LAUNCH_WORKLOAD: Workload =
    Workload { threads: "2", opens: "0", iterations: LAUNCH_ITERATIONS, warmup: 5 };

/// Accesses timed one thread at a time, from inside the target, so they price
/// interception rather than the launch around it.
const ACCESS_WORKLOAD: Workload =
    Workload { threads: "1", opens: "4096", iterations: 100, warmup: 3 };

/// The same access count spread over two threads, each opening a path of its
/// own, so that they exercise concurrent interception.
const CONCURRENT_WORKLOAD: Workload =
    Workload { threads: "2", opens: "2048", iterations: 100, warmup: 3 };

/// What a row reads out of the launches of its workload.
#[derive(Clone, Copy)]
enum Metric {
    /// Wall clock of the launch, from spawn to reap.
    Wall,
    /// The typical batch of opens, as timed inside the target.
    Typical,
    /// The slow end of those batches, which moves when accesses start to
    /// scatter rather than to cost more.
    Tail,
}

struct Row {
    name: &'static str,
    metric: Metric,
    /// The change past which the row fails the run, as a share. Set from the
    /// spread observed when benchmarking one commit on many runner instances.
    threshold: f64,
}

struct Suite {
    workload: &'static Workload,
    rows: &'static [Row],
}

const LAUNCH_SUITE: Suite = Suite {
    workload: &LAUNCH_WORKLOAD,
    rows: &[Row { name: "launch", metric: Metric::Wall, threshold: 0.05 }],
};

const ACCESS_SUITE: Suite = Suite {
    workload: &ACCESS_WORKLOAD,
    rows: &[
        Row { name: "access", metric: Metric::Typical, threshold: 0.05 },
        Row { name: "access-tail", metric: Metric::Tail, threshold: 0.08 },
    ],
};

const CONCURRENT_SUITE: Suite = Suite {
    workload: &CONCURRENT_WORKLOAD,
    rows: &[Row { name: "access-concurrent", metric: Metric::Typical, threshold: 0.08 }],
};

struct Backend {
    name: &'static str,
    target: &'static str,
}

fn main() {
    let base_launcher = parse_base_launcher();

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    let backends = [
        Backend { name: "dynamic", target: DYNAMIC_TARGET },
        Backend { name: "static", target: STATIC_TARGET },
    ];
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    let backends = [Backend { name: "dynamic", target: DYNAMIC_TARGET }];

    let mut regressed = false;
    for backend in &backends {
        validate(HEAD_LAUNCHER.as_ref(), backend.target);
        if let Some(base_launcher) = &base_launcher {
            validate(base_launcher, backend.target);
        }
        for suite in [&LAUNCH_SUITE, &ACCESS_SUITE, &CONCURRENT_SUITE] {
            regressed |= run_suite(backend, suite, base_launcher.as_deref());
        }
    }

    if regressed {
        std::process::exit(1);
    }
}

fn parse_base_launcher() -> Option<OsString> {
    let mut args = env::args_os().skip(1);
    let base_launcher = match args.next() {
        None => None,
        Some(flag) if flag == "--base-launcher" => {
            Some(args.next().expect("--base-launcher needs a path"))
        }
        Some(flag) => {
            panic!("unknown argument {}, expected --base-launcher PATH", flag.to_string_lossy())
        }
    };
    assert!(args.next().is_none(), "unexpected extra arguments");
    base_launcher
}

/// What one launch measured, in nanoseconds.
#[derive(Clone, Copy, Default)]
struct Sample {
    wall: f64,
    typical: f64,
    tail: f64,
}

impl Sample {
    const fn metric(&self, metric: Metric) -> f64 {
        match metric {
            Metric::Wall => self.wall,
            Metric::Typical => self.typical,
            Metric::Tail => self.tail,
        }
    }
}

/// One iteration's launches, one per arm.
#[derive(Default)]
struct Iteration {
    base: Sample,
    head: Sample,
    untracked: Sample,
}

/// Runs a suite's workload and reports its rows. Returns whether any row
/// regressed past its threshold.
fn run_suite(backend: &Backend, suite: &Suite, base_launcher: Option<&OsStr>) -> bool {
    let workload = suite.workload;
    let arm_count = if base_launcher.is_some() { 3 } else { 2 };
    let mut iterations = Vec::with_capacity(workload.iterations);
    for index in 0..workload.warmup + workload.iterations {
        let mut iteration = Iteration::default();
        // Rotate which arm goes first, so that no arm always pays for waking
        // the machine up or always benefits from the caches the previous arm
        // warmed.
        for position in 0..arm_count {
            match (index + position) % arm_count {
                0 => iteration.head = launch(HEAD_LAUNCHER.as_ref(), None, backend, workload),
                1 => {
                    iteration.untracked =
                        launch(HEAD_LAUNCHER.as_ref(), Some("--untracked"), backend, workload);
                }
                _ => {
                    iteration.base = launch(base_launcher.unwrap(), None, backend, workload);
                }
            }
        }
        if index >= workload.warmup {
            iterations.push(iteration);
        }
    }

    let mut regressed = false;
    for row in suite.rows {
        regressed |= report_row(backend, row, &iterations, base_launcher.is_some());
    }
    regressed
}

/// Prints one row and returns whether it regressed past its threshold.
#[expect(clippy::print_stdout, reason = "the report is the benchmark's output")]
fn report_row(backend: &Backend, row: &Row, iterations: &[Iteration], has_base: bool) -> bool {
    let name = [backend.name, "/", row.name].concat();
    let overhead = median(
        iterations
            .iter()
            .map(|it| it.head.metric(row.metric) / it.untracked.metric(row.metric) - 1.0),
    );

    if has_base {
        let mut changes: Vec<f64> = iterations
            .iter()
            .map(|it| it.head.metric(row.metric) / it.base.metric(row.metric) - 1.0)
            .collect();
        changes.sort_unstable_by(f64::total_cmp);
        let change = changes[changes.len() / 2];
        let low = changes[changes.len() / 4];
        let high = changes[changes.len() * 3 / 4];
        let verdict = if change > row.threshold {
            "  REGRESSED"
        } else if change < -row.threshold {
            "  improved"
        } else {
            ""
        };
        println!(
            "{name:<26} change {:>+6.2}%  [{:>+6.2}% .. {:>+6.2}%]  threshold {:>4.1}%  overhead {:>+8.2}%{verdict}",
            change * 100.0,
            low * 100.0,
            high * 100.0,
            row.threshold * 100.0,
            overhead * 100.0,
        );
        change > row.threshold
    } else {
        println!("{name:<26} overhead {:>+8.2}%", overhead * 100.0);
        false
    }
}

fn median(values: impl Iterator<Item = f64>) -> f64 {
    let mut values: Vec<f64> = values.collect();
    values.sort_unstable_by(f64::total_cmp);
    values[values.len() / 2]
}

/// Launches one arm and reads the three numbers its launcher printed.
fn launch(launcher: &OsStr, mode: Option<&str>, backend: &Backend, workload: &Workload) -> Sample {
    let mut command = Command::new(launcher);
    if let Some(mode) = mode {
        command.arg(mode);
    }
    let output = command
        .args([backend.target, workload.threads, workload.opens])
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .expect("failed to run the benchmark launcher");
    assert!(output.status.success(), "benchmark launcher failed: {}", output.status);
    let stdout = str::from_utf8(&output.stdout).expect("launcher output is not utf-8");
    let mut numbers = stdout
        .split_whitespace()
        .map(|number| number.parse().expect("unreadable launcher measurement"));
    let mut next = || numbers.next().expect("launcher reported too few measurements");
    Sample { wall: next(), typical: next(), tail: next() }
}

fn validate(launcher: &OsStr, target: &str) {
    let status = Command::new(launcher)
        .arg("--validate")
        .arg(target)
        .args(VALIDATION_ARGS)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .expect("failed to run the benchmark launcher");
    assert!(status.success(), "launcher validation failed for {}", launcher.to_string_lossy());
}
