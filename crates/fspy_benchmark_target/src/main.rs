use std::{
    env,
    fs::File,
    sync::{Arc, Barrier},
    thread,
};

use vite_path::{AbsolutePath, AbsolutePathBuf};

fn main() {
    let mut args = env::args_os();
    let _program = args.next();
    let root = AbsolutePathBuf::new(args.next().expect("missing fixture root").into())
        .expect("fixture root must be absolute");
    let thread_count = parse_usize(args.next(), "thread count");
    let files_per_thread = parse_usize(args.next(), "files per thread");
    assert!(args.next().is_none(), "unexpected extra arguments");

    run(&root, thread_count, files_per_thread);
}

fn parse_usize(value: Option<std::ffi::OsString>, name: &str) -> usize {
    value
        .unwrap_or_else(|| panic!("missing {name}"))
        .into_string()
        .unwrap_or_else(|_| panic!("{name} is not UTF-8"))
        .parse()
        .unwrap_or_else(|_| panic!("invalid {name}"))
}

fn run(root: &AbsolutePath, thread_count: usize, files_per_thread: usize) {
    assert!(thread_count > 1, "benchmark requires multiple threads");
    assert!(files_per_thread > 0, "benchmark requires at least one file per thread");

    let paths = (0..thread_count * files_per_thread)
        .map(|index| root.join(vite_str::format!("file-{index:05}.txt")))
        .collect::<Vec<_>>();
    let barrier = Arc::new(Barrier::new(thread_count));

    thread::scope(|scope| {
        for thread_paths in paths.chunks_exact(files_per_thread) {
            let barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                barrier.wait();
                for path in thread_paths {
                    drop(File::open(path).unwrap_or_else(|error| {
                        panic!("failed to open {}: {error}", path.as_absolute_path())
                    }));
                }
            });
        }
    });
}
