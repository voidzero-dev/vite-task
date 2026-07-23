use std::{
    ffi::OsStr,
    fs::File,
    process::{ExitStatus, Stdio},
    time::Duration,
};

use criterion::{
    BenchmarkGroup, BenchmarkId, Criterion, SamplingMode, criterion_group, criterion_main,
    measurement::WallTime,
};
use fspy::{ChildTermination, Command};
use tempfile::TempDir;
use tokio::runtime::{Builder, Runtime};
use tokio_util::sync::CancellationToken;
use vite_path::{AbsolutePath, AbsolutePathBuf};

const THREAD_COUNT: usize = 4;
const FILES_PER_THREAD: usize = 2048;
const TOTAL_FILE_COUNT: usize = THREAD_COUNT * FILES_PER_THREAD;
const THREAD_COUNT_ARG: &str = "4";
const FILES_PER_THREAD_ARG: &str = "2048";
const DYNAMIC_TARGET: &str = env!("CARGO_BIN_FILE_FSPY_BENCHMARK_TARGET");

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const STATIC_TARGET: &str = env!("CARGO_BIN_FILE_FSPY_BENCHMARK_STATIC_TARGET");

fn benchmark(criterion: &mut Criterion) {
    let (_fixture, fixture_root) = create_fixture();
    let runtime = Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let mut group = criterion.benchmark_group("fspy");
    group.sampling_mode(SamplingMode::Flat);

    benchmark_target(&mut group, &runtime, "dynamic", DYNAMIC_TARGET, &fixture_root);

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    benchmark_target(&mut group, &runtime, "static", STATIC_TARGET, &fixture_root);

    group.finish();
}

fn benchmark_target(
    group: &mut BenchmarkGroup<'_, WallTime>,
    runtime: &Runtime,
    target_name: &str,
    target: &str,
    fixture: &AbsolutePath,
) {
    validate_tracked_run(runtime, target, fixture);

    group.bench_with_input(
        BenchmarkId::new(target_name, "untracked"),
        &target,
        |bencher, target| {
            bencher.iter_with_setup_wrapper(|runner| {
                let status = runner.run(|| runtime.block_on(run_untracked(target, fixture)));
                assert!(status.success(), "untracked benchmark target failed: {status}");
            });
        },
    );

    group.bench_with_input(BenchmarkId::new(target_name, "tracked"), &target, |bencher, target| {
        bencher.iter_with_setup_wrapper(|runner| {
            let termination = runner.run(|| runtime.block_on(run_tracked(target, fixture)));
            assert!(
                termination.status.success(),
                "tracked benchmark target failed: {}",
                termination.status
            );
        });
    });
}

fn create_fixture() -> (TempDir, AbsolutePathBuf) {
    let fixture = tempfile::tempdir().expect("failed to create benchmark fixture");
    let fixture_root = AbsolutePathBuf::new(fixture.path().to_path_buf())
        .expect("temporary path must be absolute");
    for index in 0..TOTAL_FILE_COUNT {
        let path = fixture_root.join(vite_str::format!("file-{index:05}.txt"));
        File::create(&path).unwrap_or_else(|error| {
            panic!("failed to create {}: {error}", path.as_absolute_path())
        });
    }
    (fixture, fixture_root)
}

fn validate_tracked_run(runtime: &Runtime, target: &str, fixture: &AbsolutePath) {
    let termination = runtime.block_on(run_tracked(target, fixture));
    assert!(termination.status.success(), "benchmark target failed: {}", termination.status);
    let fixture_access_count = termination
        .path_accesses
        .iter()
        .filter(|access| access.path.strip_path_prefix(fixture, |result| result.is_ok()))
        .count();
    assert!(
        fixture_access_count >= TOTAL_FILE_COUNT,
        "expected at least {TOTAL_FILE_COUNT} fixture accesses, captured {fixture_access_count}"
    );
}

async fn run_untracked(target: &str, fixture: &AbsolutePath) -> ExitStatus {
    let mut command = tokio::process::Command::new(target);
    command
        .env_clear()
        .args(benchmark_args(fixture))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.status().await.expect("failed to run untracked benchmark target")
}

async fn run_tracked(target: &str, fixture: &AbsolutePath) -> ChildTermination {
    let mut command = Command::new(target);
    command
        .args(benchmark_args(fixture))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
        .spawn(CancellationToken::new())
        .await
        .expect("failed to spawn tracked benchmark target")
        .wait_handle
        .await
        .expect("failed to wait for tracked benchmark target")
}

fn benchmark_args(fixture: &AbsolutePath) -> [&OsStr; 3] {
    [fixture.as_path().as_os_str(), OsStr::new(THREAD_COUNT_ARG), OsStr::new(FILES_PER_THREAD_ARG)]
}

fn criterion_config() -> Criterion {
    Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_secs(2))
        .measurement_time(Duration::from_secs(10))
}

criterion_group! {
    name = benches;
    config = criterion_config();
    targets = benchmark
}
criterion_main!(benches);
