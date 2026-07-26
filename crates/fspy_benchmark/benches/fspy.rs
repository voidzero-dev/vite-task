use std::{
    cell::RefCell,
    collections::VecDeque,
    process::Stdio,
    time::{Duration, Instant},
};

use criterion::{
    BenchmarkGroup, BenchmarkId, Criterion, SamplingMode, Throughput, criterion_group,
    criterion_main,
    measurement::{Measurement, ValueFormatter},
};
use fspy::{ChildTermination, Command};
use tokio::{
    io::AsyncReadExt as _,
    process::ChildStdout,
    runtime::{Builder, Runtime},
};
use tokio_util::sync::CancellationToken;

const DYNAMIC_TARGET: &str = env!("CARGO_BIN_FILE_FSPY_BENCHMARK_TARGET");

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const STATIC_TARGET: &str = env!("CARGO_BIN_FILE_FSPY_BENCHMARK_STATIC_TARGET");

#[cfg(unix)]
const MISSING_PATH: &str = "/.fspy-benchmark-missing";
#[cfg(windows)]
const MISSING_PATH: &str = r"C:\.fspy-benchmark-missing";

struct Workload {
    threads: &'static str,
    opens: &'static str,
    /// Launch pairs behind every Criterion sample. Fixed so that a sample always
    /// averages the same amount of work, whatever the runner speed is.
    pairs_per_sample: u32,
}

/// Concurrent launches scatter far more than single threaded ones, and by how
/// much depends on how many cores the runner has to spare, so samples have to
/// average enough pairs for the median to settle. Unix launches are cheap
/// enough to afford four times the base count; Windows launches cost several
/// times more wall clock and reproduce just as well without it.
#[cfg(windows)]
const PAIR_MULTIPLIER: u32 = 1;
#[cfg(not(windows))]
const PAIR_MULTIPLIER: u32 = 4;

/// Opens one batch, so that capturing can be validated before measuring. Never
/// sampled, so its pair count does not matter.
const VALIDATION_WORKLOAD: Workload = Workload { threads: "1", opens: "8", pairs_per_sample: 0 };

/// Opens nothing, so the whole launch is the cost of starting a tracked
/// process. Two threads, because a process that fspy tracks rarely has one.
const LAUNCH_WORKLOAD: Workload = Workload { threads: "2", opens: "0", pairs_per_sample: 200 };

/// Accesses timed one thread at a time. Its samples come from inside the
/// target, so a handful of launches already carries hundreds of them.
const ACCESS_WORKLOAD: Workload = Workload { threads: "1", opens: "4096", pairs_per_sample: 25 };

/// The same access count spread over two threads, each opening a path of its
/// own so that they exercise interception rather than one shared lookup.
#[cfg(not(target_os = "macos"))]
const CONCURRENT_ACCESS_WORKLOAD: Workload =
    Workload { threads: "2", opens: "2048", pairs_per_sample: 25 };

/// fspy costs a process two separable things: a one-off cost to start it under
/// tracking, and a cost on every access it makes. They are measured apart so
/// that neither dilutes the other, and each in the way that reproduces best.
#[derive(Clone, Copy)]
enum Metric {
    /// Wall clock of a launch that opens nothing.
    Launch = 0,
    /// The typical time a batch of opens took, timed by the thread running it.
    Access = 1,
    /// The slow end of the same samples, which moves when accesses start to
    /// scatter rather than to cost more.
    AccessTail = 2,
    /// The typical sample once a second thread is intercepting as well.
    ConcurrentAccess = 3,
}

impl Metric {
    const fn name(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Access => "access",
            Self::AccessTail => "access-tail",
            Self::ConcurrentAccess => "access-concurrent",
        }
    }

    const fn of(self, launch: &Launch) -> Duration {
        match self {
            Self::Launch => launch.wall,
            Self::Access | Self::ConcurrentAccess => launch.access,
            Self::AccessTail => launch.access_tail,
        }
    }
}

/// A workload and the metrics read out of its launches.
struct Case {
    workload: &'static Workload,
    metrics: &'static [Metric],
}

/// Two threads racing to be intercepted only reproduce where the runner can be
/// counted on to run them at once. On macOS the same measurement moves by 25%
/// between runner instances, against 3% for one thread, because a second busy
/// thread there makes every open cost about three times as much by an amount
/// that changes from launch to launch.
#[cfg(target_os = "macos")]
const CONCURRENT_ACCESS_CASES: &[Case] = &[];
#[cfg(not(target_os = "macos"))]
const CONCURRENT_ACCESS_CASES: &[Case] =
    &[Case { workload: &CONCURRENT_ACCESS_WORKLOAD, metrics: &[Metric::ConcurrentAccess] }];

const ACCESS_CASES: &[Case] =
    &[Case { workload: &ACCESS_WORKLOAD, metrics: &[Metric::Access, Metric::AccessTail] }];

const LAUNCH_CASES: &[Case] = &[Case { workload: &LAUNCH_WORKLOAD, metrics: &[Metric::Launch] }];

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const STATIC_ACCESS_CASES: &[Case] =
    &[Case { workload: &ACCESS_WORKLOAD, metrics: &[Metric::Access] }];

fn benchmark(criterion: &mut Criterion<Overhead>) {
    let runtime = Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();

    let mut group = criterion.benchmark_group("fspy");
    group.sampling_mode(SamplingMode::Flat);

    for cases in [LAUNCH_CASES, ACCESS_CASES, CONCURRENT_ACCESS_CASES] {
        benchmark_target(&mut group, &runtime, "dynamic", DYNAMIC_TARGET, cases);
    }

    // Setting up seccomp user notification costs too differently between runner
    // instances to compare, so the static target only measures accesses. Its
    // tail is left out too: waiting for the supervisor to wake up scatters
    // enough that the slow end moves by 10% between runner instances.
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    for cases in [STATIC_ACCESS_CASES, CONCURRENT_ACCESS_CASES] {
        benchmark_target(&mut group, &runtime, "static", STATIC_TARGET, cases);
    }

    group.finish();
}

fn benchmark_target(
    group: &mut BenchmarkGroup<'_, Overhead>,
    runtime: &Runtime,
    target_name: &str,
    target: &str,
    cases: &[Case],
) {
    validate_tracked_run(runtime, target);

    for case in cases {
        benchmark_case(group, runtime, target_name, target, case);
    }
}

fn benchmark_case(
    group: &mut BenchmarkGroup<'_, Overhead>,
    runtime: &Runtime,
    target_name: &str,
    target: &str,
    case: &Case,
) {
    let workload = case.workload;
    let metrics = case.metrics;

    // Criterion reports one number per benchmark and runs benchmarks one after
    // another, so the first one measures every metric of the case and leaves
    // the samples of the metrics after it here.
    let handed_over: RefCell<VecDeque<f64>> = RefCell::new(VecDeque::new());

    for (position, &metric) in metrics.iter().enumerate() {
        group.bench_function(BenchmarkId::new(target_name, metric.name()), |bencher| {
            bencher.iter_custom(|iterations| {
                let overhead = if position == 0 {
                    let overheads = measure_overheads(runtime, target, workload);
                    let mut handed_over = handed_over.borrow_mut();
                    for later in &metrics[1..] {
                        handed_over.push_back(overheads[*later as usize]);
                    }
                    overheads[metric as usize]
                } else {
                    let sample = handed_over.borrow_mut().pop_front();
                    sample.unwrap_or_else(|| {
                        measure_overheads(runtime, target, workload)[metric as usize]
                    })
                };
                // The sample always covers `pairs_per_sample` launch pairs, so
                // report the per-pair overhead scaled to whatever iteration
                // count Criterion asked for.
                #[expect(clippy::cast_precision_loss, reason = "iteration counts stay small")]
                let iterations = iterations as f64;
                overhead * iterations
            });
        });
    }
}

/// Interleaves tracked and untracked launches, alternating which one goes first
/// so that ordering cannot bias a pair, and reports the median per-pair
/// overhead of every metric.
///
/// Absolute launch times differ by tens of percent between runner instances,
/// while the overhead of a tracked launch relative to an untracked launch
/// measured back to back stays comparable, which is what makes results from
/// different runs comparable. Overhead is reported relative to the untracked
/// launch instead of as a tracked/untracked ratio so that a given slowdown in
/// fspy moves the number by the same amount on every platform, however cheap
/// tracking is compared with the work the target does.
fn measure_overheads(runtime: &Runtime, target: &str, workload: &Workload) -> [f64; 4] {
    let pairs = workload.pairs_per_sample * PAIR_MULTIPLIER;
    let mut overheads = [
        Vec::with_capacity(pairs as usize),
        Vec::with_capacity(pairs as usize),
        Vec::with_capacity(pairs as usize),
        Vec::with_capacity(pairs as usize),
    ];
    for index in 0..pairs {
        let (tracked, untracked) = if index % 2 == 0 {
            let untracked = measure(runtime, target, workload, Tracking::Off);
            (measure(runtime, target, workload, Tracking::On), untracked)
        } else {
            let tracked = measure(runtime, target, workload, Tracking::On);
            (tracked, measure(runtime, target, workload, Tracking::Off))
        };
        for (metric, samples) in
            [Metric::Launch, Metric::Access, Metric::AccessTail, Metric::ConcurrentAccess]
                .into_iter()
                .zip(&mut overheads)
        {
            samples.push(overhead(metric, &tracked, &untracked));
        }
    }
    overheads.map(|mut samples| median(&mut samples))
}

fn overhead(metric: Metric, tracked: &Launch, untracked: &Launch) -> f64 {
    metric.of(tracked).as_secs_f64() / metric.of(untracked).as_secs_f64() - 1.0
}

fn median(overheads: &mut [f64]) -> f64 {
    overheads.sort_unstable_by(f64::total_cmp);
    overheads[overheads.len() / 2]
}

#[derive(Clone, Copy)]
enum Tracking {
    On,
    Off,
}

/// What one launch measured.
struct Launch {
    /// Wall clock of the launch, from spawning it to reaping it.
    wall: Duration,
    /// The typical batch of opens, as timed inside the target.
    access: Duration,
    /// The slow end of those batches.
    access_tail: Duration,
}

fn validate_tracked_run(runtime: &Runtime, target: &str) {
    let termination = runtime.block_on(run_tracked(target, &VALIDATION_WORKLOAD));
    assert!(termination.status.success(), "benchmark target failed: {}", termination.status);
    let captured_missing_access = termination.path_accesses.iter().any(|access| {
        access.path.strip_path_prefix(MISSING_PATH, |result| {
            result.is_ok_and(|path| path.as_os_str().is_empty())
        })
    });
    assert!(captured_missing_access, "failed to capture access to {MISSING_PATH}");
}

fn measure(runtime: &Runtime, target: &str, workload: &Workload, tracking: Tracking) -> Launch {
    runtime.block_on(async {
        let start = Instant::now();
        let mut reported = match tracking {
            Tracking::Off => run_untracked(target, workload).await,
            Tracking::On => run_tracked_reporting(target, workload).await,
        };
        let wall = start.elapsed();
        let [access, access_tail] = read_reported_samples(&mut reported).await;
        Launch { wall, access, access_tail }
    })
}

/// Reads the samples the target timed for itself, which it writes to its
/// standard output once it is done.
async fn read_reported_samples(reported: &mut ChildStdout) -> [Duration; 2] {
    let mut output = Vec::new();
    reported.read_to_end(&mut output).await.expect("failed to read the reported samples");
    let output = str::from_utf8(&output).expect("the reported samples are not utf-8");
    let mut samples = output
        .split_whitespace()
        .map(|sample| Duration::from_nanos(sample.parse().expect("unreadable reported sample")));
    let typical = samples.next().expect("the target reported no samples");
    let tail = samples.next().expect("the target reported no tail sample");
    [typical, tail]
}

async fn run_untracked(target: &str, workload: &Workload) -> ChildStdout {
    let mut child = tokio::process::Command::new(target)
        .args([workload.threads, workload.opens])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("failed to run untracked benchmark target");
    let stdout = child.stdout.take().expect("untracked benchmark target has no stdout");
    let status = child.wait().await.expect("failed to wait for untracked benchmark target");
    assert!(status.success(), "untracked benchmark target failed: {status}");
    stdout
}

async fn run_tracked_reporting(target: &str, workload: &Workload) -> ChildStdout {
    let mut child = tracked_command(target, workload, Stdio::piped())
        .spawn(CancellationToken::new())
        .await
        .expect("failed to spawn tracked benchmark target");
    let stdout = child.stdout.take().expect("tracked benchmark target has no stdout");
    let termination = child.wait_handle.await.expect("failed to wait for tracked target");
    assert!(
        termination.status.success(),
        "tracked benchmark target failed: {}",
        termination.status
    );
    stdout
}

async fn run_tracked(target: &str, workload: &Workload) -> ChildTermination {
    tracked_command(target, workload, Stdio::null())
        .spawn(CancellationToken::new())
        .await
        .expect("failed to spawn tracked benchmark target")
        .wait_handle
        .await
        .expect("failed to wait for tracked benchmark target")
}

fn tracked_command(target: &str, workload: &Workload, stdout: Stdio) -> Command {
    let mut command = Command::new(target);
    command
        .args([workload.threads, workload.opens])
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::inherit());
    command
}

/// Measures the share of extra wall-clock time a tracked launch takes over an
/// untracked one.
struct Overhead;

impl Measurement for Overhead {
    type Intermediate = ();
    type Value = f64;

    fn start(&self) -> Self::Intermediate {}

    fn end(&self, _intermediate: Self::Intermediate) -> Self::Value {
        // Samples come from `iter_custom`, which reports shares directly.
        0.0
    }

    fn add(&self, v1: &Self::Value, v2: &Self::Value) -> Self::Value {
        v1 + v2
    }

    fn zero(&self) -> Self::Value {
        0.0
    }

    fn to_f64(&self, value: &Self::Value) -> f64 {
        *value
    }

    fn formatter(&self) -> &dyn ValueFormatter {
        &PercentFormatter
    }
}

struct PercentFormatter;

impl PercentFormatter {
    fn to_percent(values: &mut [f64]) -> &'static str {
        for value in values {
            *value *= 100.0;
        }
        "%"
    }
}

impl ValueFormatter for PercentFormatter {
    fn scale_values(&self, _typical_value: f64, values: &mut [f64]) -> &'static str {
        Self::to_percent(values)
    }

    fn scale_throughputs(
        &self,
        _typical_value: f64,
        _throughput: &Throughput,
        values: &mut [f64],
    ) -> &'static str {
        Self::to_percent(values)
    }

    fn scale_for_machines(&self, values: &mut [f64]) -> &'static str {
        Self::to_percent(values)
    }
}

fn criterion_config() -> Criterion<Overhead> {
    Criterion::default()
        .with_measurement(Overhead)
        .sample_size(10)
        // A single call already covers a full sample of launch pairs, and the
        // benchmark that reads handed over samples must not spend its warmup
        // draining them.
        .warm_up_time(Duration::from_nanos(1))
        .measurement_time(Duration::from_secs(5))
        // Runner instances still disagree by a few percent, so only report
        // larger moves as changes.
        .noise_threshold(0.1)
}

criterion_group! {
    name = benches;
    config = criterion_config();
    targets = benchmark
}
criterion_main!(benches);
