use std::{
    fs::File,
    sync::{Arc, Barrier},
    thread,
};

const THREAD_COUNT: usize = 4;
const OPEN_COUNT_PER_THREAD: usize = 8_192;

#[cfg(unix)]
const MISSING_PATH: &str = "/.fspy-benchmark-missing";
#[cfg(windows)]
const MISSING_PATH: &str = r"C:\.fspy-benchmark-missing";

fn main() {
    let barrier = Arc::new(Barrier::new(THREAD_COUNT));

    thread::scope(|scope| {
        for _ in 0..THREAD_COUNT {
            let barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                barrier.wait();
                for _ in 0..OPEN_COUNT_PER_THREAD {
                    drop(File::open(MISSING_PATH));
                }
            });
        }
    });
}
