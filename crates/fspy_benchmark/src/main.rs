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
//! untracked — back to back, cycling every ordering, and yields the ratio of
//! head to baseline per metric. The reported change is the median of those
//! ratios, so it prices exactly one thing: what the fspy change under review
//! costs a tracked process. The untracked arm prices tracking itself, as
//! context.

use std::{
    env,
    ffi::{OsStr, OsString},
    process::{Command, Stdio},
};

const HEAD_LAUNCHER: &str = env!("CARGO_BIN_FILE_FSPY_BENCHMARK_LAUNCHER");
const DYNAMIC_TARGET: &str = env!("CARGO_BIN_FILE_FSPY_BENCHMARK_TARGET");
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const STATIC_TARGET: &str = env!("CARGO_BIN_FILE_FSPY_BENCHMARK_STATIC_TARGET");

/// What a suite reads out of its launches.
#[derive(Clone, Copy)]
enum Metric {
    /// Wall clock of the launch, from spawn to reap.
    Wall,
    /// The typical batch of opens, as timed inside the target.
    Typical,
}

struct Suite {
    name: &'static str,
    /// Threads the target runs, passed through to it. Two for most suites,
    /// because a process that fspy tracks rarely accesses files from one
    /// thread.
    threads: &'static str,
    /// Opens per target thread, passed through to it.
    opens: &'static str,
    /// Measured iterations. Each one launches every arm once.
    iterations: usize,
    /// Unmeasured iterations run first, to fill caches and settle the runner.
    warmup: usize,
    metric: Metric,
    /// Whether the target opens a relative path (the launcher's
    /// `--relative`), driving the tracker's working-directory resolution and
    /// path joining instead of the borrow-only absolute lane.
    relative: bool,
}

/// Opens nothing, so the whole launch is the cost of starting a tracked
/// process. Launches cost several times more wall clock on Windows, so it
/// affords fewer of them in the same time.
const LAUNCH_SUITE: Suite = Suite {
    name: "launch",
    threads: "2",
    opens: "0",
    iterations: if cfg!(windows) { 150 } else { 300 },
    warmup: 5,
    metric: Metric::Wall,
    relative: false,
};

/// Opens timed from inside the target, so they price interception rather than
/// the launch around it. The absolute path takes the tracker's borrow-only
/// lane; the relative variant prices working-directory resolution and path
/// joining on top.
const ACCESS_SUITE: Suite = Suite {
    name: "access",
    threads: "2",
    opens: "2048",
    iterations: 102,
    warmup: 3,
    metric: Metric::Typical,
    relative: false,
};

const RELATIVE_ACCESS_SUITE: Suite = Suite {
    name: "access-relative",
    threads: "2",
    opens: "2048",
    iterations: 102,
    warmup: 3,
    metric: Metric::Typical,
    relative: true,
};

/// The same opens under enough threads to make them fight over the tracker's
/// shared counters. Each record costs two atomic read-modify-writes on words
/// every other thread is also touching, so this is the row that prices how
/// that contention scales; the two-thread rows barely provoke it. Runs half
/// the iterations of the plain suite over half the opens, so four times the
/// threads cost about the same wall clock.
const CONTENDED_ACCESS_SUITE: Suite = Suite {
    name: "access-contended",
    threads: "8",
    opens: "1024",
    iterations: 54,
    warmup: 3,
    metric: Metric::Typical,
    relative: false,
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

    for backend in &backends {
        for relative in [false, true] {
            validate(HEAD_LAUNCHER.as_ref(), backend.target, relative);
            if let Some(base_launcher) = &base_launcher {
                validate(base_launcher, backend.target, relative);
            }
        }
        for suite in [&LAUNCH_SUITE, &ACCESS_SUITE, &RELATIVE_ACCESS_SUITE, &CONTENDED_ACCESS_SUITE]
        {
            run_suite(backend, suite, base_launcher.as_deref());
        }
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

/// One iteration's measurements, one per arm, in nanoseconds.
#[derive(Default)]
struct Iteration {
    base: f64,
    head: f64,
    untracked: f64,
}

/// Every ordering of the arms of an iteration. Cycling through all of them —
/// rather than rotating one fixed cycle, which keeps every arm's neighbors
/// the same forever — evens out both what position an arm runs in and which
/// arm's leftovers it runs after.
const ORDERS_WITHOUT_BASE: &[&[Arm]] =
    &[&[Arm::Head, Arm::Untracked], &[Arm::Untracked, Arm::Head]];
const ORDERS_WITH_BASE: &[&[Arm]] = &[
    &[Arm::Head, Arm::Untracked, Arm::Base],
    &[Arm::Untracked, Arm::Base, Arm::Head],
    &[Arm::Base, Arm::Head, Arm::Untracked],
    &[Arm::Base, Arm::Untracked, Arm::Head],
    &[Arm::Untracked, Arm::Head, Arm::Base],
    &[Arm::Head, Arm::Base, Arm::Untracked],
];

#[derive(Clone, Copy)]
enum Arm {
    Head,
    Untracked,
    Base,
}

/// Runs a suite and reports its row.
fn run_suite(backend: &Backend, suite: &Suite, base_launcher: Option<&OsStr>) {
    let orders = if base_launcher.is_some() { ORDERS_WITH_BASE } else { ORDERS_WITHOUT_BASE };
    // Whole cycles only, so that each ordering runs equally often.
    let iterations = suite.iterations.next_multiple_of(orders.len());
    let mut measured = Vec::with_capacity(iterations);
    for index in 0..suite.warmup + iterations {
        let mut iteration = Iteration::default();
        for &arm in orders[index % orders.len()] {
            match arm {
                Arm::Head => {
                    iteration.head = launch(HEAD_LAUNCHER.as_ref(), None, backend, suite);
                }
                Arm::Untracked => {
                    iteration.untracked =
                        launch(HEAD_LAUNCHER.as_ref(), Some("--untracked"), backend, suite);
                }
                Arm::Base => {
                    iteration.base = launch(base_launcher.unwrap(), None, backend, suite);
                }
            }
        }
        if index >= suite.warmup {
            measured.push(iteration);
        }
    }

    report(backend, suite, &measured, base_launcher.is_some());
}

/// Prints the suite's row.
#[expect(clippy::print_stdout, reason = "the report is the benchmark's output")]
fn report(backend: &Backend, suite: &Suite, iterations: &[Iteration], has_base: bool) {
    let name = [backend.name, "/", suite.name].concat();
    let overheads = sorted(iterations.iter().map(|it| it.head / it.untracked - 1.0));
    let overhead = quantile(&overheads, 1, 2);

    if has_base {
        let changes = sorted(iterations.iter().map(|it| it.head / it.base - 1.0));
        println!(
            "{name:<26} change {:>+6.2}%  [{:>+6.2}% .. {:>+6.2}%]  overhead {:>+8.2}%",
            quantile(&changes, 1, 2) * 100.0,
            quantile(&changes, 1, 4) * 100.0,
            quantile(&changes, 3, 4) * 100.0,
            overhead * 100.0,
        );
    } else {
        println!("{name:<26} overhead {:>+8.2}%", overhead * 100.0);
    }
}

fn sorted(values: impl Iterator<Item = f64>) -> Vec<f64> {
    let mut values: Vec<f64> = values.collect();
    values.sort_unstable_by(f64::total_cmp);
    values
}

const fn quantile(sorted: &[f64], numerator: usize, denominator: usize) -> f64 {
    sorted[sorted.len() * numerator / denominator]
}

/// Launches one arm and reads the metric the suite names out of the two
/// numbers its launcher printed.
fn launch(launcher: &OsStr, mode: Option<&str>, backend: &Backend, suite: &Suite) -> f64 {
    let mut command = Command::new(launcher);
    if let Some(mode) = mode {
        command.arg(mode);
    }
    if suite.relative {
        command.arg("--relative");
    }
    let output = command
        .args([backend.target, suite.threads, suite.opens])
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
    let [wall, typical] = [next(), next()];
    match suite.metric {
        Metric::Wall => wall,
        Metric::Typical => typical,
    }
}

fn validate(launcher: &OsStr, target: &str, relative: bool) {
    let mut command = Command::new(launcher);
    command.arg("--validate");
    if relative {
        command.arg("--relative");
    }
    let status = command
        .arg(target)
        // One thread opening one batch: the count matches the target's
        // OPENS_PER_SAMPLE. Fewer would open nothing, which validation
        // catches by failing to capture the access.
        .args(["1", "8"])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .expect("failed to run the benchmark launcher");
    assert!(status.success(), "launcher validation failed for {}", launcher.to_string_lossy());
}
