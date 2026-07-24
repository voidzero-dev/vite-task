use std::{
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use criterion::{
    BenchmarkGroup, BenchmarkId, Criterion, SamplingMode, criterion_group, criterion_main,
    measurement::WallTime,
};
use fspy::{ChildTermination, Command};
use tokio::runtime::{Builder, Runtime};
use tokio_util::sync::CancellationToken;

const DYNAMIC_TARGET: &str = env!("CARGO_BIN_FILE_FSPY_BENCHMARK_TARGET");

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const STATIC_TARGET: &str = env!("CARGO_BIN_FILE_FSPY_BENCHMARK_STATIC_TARGET");

#[cfg(unix)]
const MISSING_PATH: &str = "/.fspy-benchmark-missing";
#[cfg(windows)]
const MISSING_PATH: &str = r"C:\.fspy-benchmark-missing";

fn benchmark(criterion: &mut Criterion) {
    let runtime = Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let mut group = criterion.benchmark_group("fspy");
    group.sampling_mode(SamplingMode::Flat);

    benchmark_target(&mut group, &runtime, "dynamic", DYNAMIC_TARGET);

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    benchmark_target(&mut group, &runtime, "static", STATIC_TARGET);

    group.finish();
}

fn benchmark_target(
    group: &mut BenchmarkGroup<'_, WallTime>,
    runtime: &Runtime,
    target_name: &str,
    target: &str,
) {
    validate_tracked_run(runtime, target);

    group.bench_with_input(BenchmarkId::from_parameter(target_name), &target, |bencher, target| {
        bencher.iter_custom(|iterations| {
            let mut tracked = Duration::ZERO;
            let mut untracked = Duration::ZERO;
            for _ in 0..iterations {
                untracked += measure_untracked(runtime, target);
                tracked += measure_tracked(runtime, target);
            }
            // Keep clamped samples reportable at a stable 1 ns per-iteration floor.
            tracked.saturating_sub(untracked).max(Duration::from_nanos(iterations))
        });
    });
}

fn validate_tracked_run(runtime: &Runtime, target: &str) {
    let termination = runtime.block_on(run_tracked(target));
    assert!(termination.status.success(), "benchmark target failed: {}", termination.status);
    let captured_missing_access = termination.path_accesses.iter().any(|access| {
        access.path.strip_path_prefix(MISSING_PATH, |result| {
            result.is_ok_and(|path| path.as_os_str().is_empty())
        })
    });
    assert!(captured_missing_access, "failed to capture access to {MISSING_PATH}");
}

fn measure_untracked(runtime: &Runtime, target: &str) -> Duration {
    let start = Instant::now();
    let status = runtime.block_on(run_untracked(target));
    let elapsed = start.elapsed();
    assert!(status.success(), "untracked benchmark target failed: {status}");
    elapsed
}

fn measure_tracked(runtime: &Runtime, target: &str) -> Duration {
    let start = Instant::now();
    let termination = runtime.block_on(run_tracked(target));
    let elapsed = start.elapsed();
    assert!(
        termination.status.success(),
        "tracked benchmark target failed: {}",
        termination.status
    );
    elapsed
}

async fn run_untracked(target: &str) -> ExitStatus {
    let mut command = tokio::process::Command::new(target);
    command.env_clear().stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit());
    command.status().await.expect("failed to run untracked benchmark target")
}

async fn run_tracked(target: &str) -> ChildTermination {
    let mut command = Command::new(target);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit());
    command
        .spawn(CancellationToken::new())
        .await
        .expect("failed to spawn tracked benchmark target")
        .wait_handle
        .await
        .expect("failed to wait for tracked benchmark target")
}

fn criterion_config() -> Criterion {
    Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_secs(5))
        .measurement_time(Duration::from_secs(60))
}

criterion_group! {
    name = benches;
    config = criterion_config();
    targets = benchmark
}
criterion_main!(benches);
