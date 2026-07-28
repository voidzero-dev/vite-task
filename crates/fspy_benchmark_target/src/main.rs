use std::{
    env,
    fs::File,
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

const DEFAULT_THREAD_COUNT: usize = 4;
const DEFAULT_OPEN_COUNT_PER_THREAD: usize = 512;

/// Opens behind one timing sample. Timing every open on its own would leave the
/// sample close to the clock's granularity, which is 42 ns on Apple silicon and
/// 100 ns on Windows, while a batch this small still keeps a sample short
/// enough that the scheduler only ever spoils a few of them.
const OPENS_PER_SAMPLE: usize = 8;

#[cfg(unix)]
const MISSING_PATH: &str = "/.fspy-benchmark-missing";
#[cfg(windows)]
const MISSING_PATH: &str = r"C:\.fspy-benchmark-missing";

/// Threads open distinct paths so that they do not contend for the same lookup.
const THREAD_SUFFIXES: [&str; 8] = ["", "1", "2", "3", "4", "5", "6", "7"];

fn main() {
    let mut args = env::args().skip(1);
    let thread_count =
        args.next().and_then(|arg| arg.parse().ok()).unwrap_or(DEFAULT_THREAD_COUNT).max(1);
    let open_count_per_thread =
        args.next().and_then(|arg| arg.parse().ok()).unwrap_or(DEFAULT_OPEN_COUNT_PER_THREAD);
    let barrier = Arc::new(Barrier::new(thread_count));

    let mut samples = Vec::new();
    thread::scope(|scope| {
        let workers: Vec<_> = (0..thread_count)
            .map(|index| {
                let barrier = Arc::clone(&barrier);
                let mut path = MISSING_PATH.to_owned();
                path.push_str(THREAD_SUFFIXES[index % THREAD_SUFFIXES.len()]);
                scope.spawn(move || time_opens(&barrier, &path, open_count_per_thread))
            })
            .collect();
        for worker in workers {
            samples.extend(worker.join().expect("benchmark thread panicked"));
        }
    });

    report(&mut samples);
}

/// Times how long batches of opens take, from the thread that runs them, so
/// that the measurement covers the opens themselves rather than the launch
/// around them.
fn time_opens(barrier: &Barrier, path: &str, open_count: usize) -> Vec<Duration> {
    let sample_count = open_count / OPENS_PER_SAMPLE;
    let mut samples = Vec::with_capacity(sample_count);
    barrier.wait();
    for _ in 0..sample_count {
        let started = Instant::now();
        for _ in 0..OPENS_PER_SAMPLE {
            drop(File::open(path));
        }
        samples.push(started.elapsed());
    }
    samples
}

/// Reports the typical sample. The median discards the batches the scheduler
/// spoiled, which a whole-run wall clock would sum up instead.
fn report(samples: &mut [Duration]) {
    samples.sort_unstable();
    let median = samples.get(samples.len() / 2).map_or(0, Duration::as_nanos);
    #[expect(clippy::print_stdout, reason = "the benchmark reads the samples from stdout")]
    {
        println!("{median}");
    }
}
