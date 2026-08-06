use std::{
    env,
    fs::File,
    sync::Barrier,
    thread,
    time::{Duration, Instant},
};

/// Opens behind one timing sample. Timing every open on its own would leave the
/// sample close to the clock's granularity, which is 42 ns on Apple silicon and
/// 100 ns on Windows, while a batch this small still keeps a sample short
/// enough that the scheduler only ever spoils a few of them.
const OPENS_PER_SAMPLE: usize = 8;

fn main() {
    let mut args = env::args().skip(1);
    let mut next = || args.next().expect("usage: fspy_benchmark_target THREADS OPENS PATH");
    let thread_count: usize = next().parse().expect("unreadable thread count");
    let open_count_per_thread: usize = next().parse().expect("unreadable open count");
    let base_path = next();

    let barrier = &Barrier::new(thread_count);
    let mut samples = Vec::with_capacity(thread_count * (open_count_per_thread / OPENS_PER_SAMPLE));
    thread::scope(|scope| {
        let workers: Vec<_> = (0..thread_count)
            .map(|index| {
                // Threads open distinct paths so that they do not contend for
                // the same lookup. The first thread opens the bare path, which
                // is the one the launcher validates was captured.
                let mut path = base_path.clone();
                if index > 0 {
                    path.push_str(&index.to_string());
                }
                scope.spawn(move || time_opens(barrier, &path, open_count_per_thread))
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
