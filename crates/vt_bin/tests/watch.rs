//! Real process tests for persistent sessions. Synchronize on milestones and
//! file contents, rather than assuming commands finish within a fixed delay.
#![expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    reason = "test harness uses std paths, strings, and environment setup"
)]
use std::{
    io::Write as _,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

use pty_terminal_test::{CommandBuilder, ScreenSize, TestTerminal};
use serde_json::json;

fn fixture(config: &serde_json::Value) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "package.json", r#"{"name":"watch-test"}"#);
    write(directory.path(), "vite-task.json", &config.to_string());
    directory
}

fn write(root: &Path, name: &str, value: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value).unwrap();
}

fn binary_path() -> std::ffi::OsString {
    std::env::join_paths(
        std::iter::once(PathBuf::from(env!("CARGO_BIN_EXE_vtt")).parent().unwrap().to_owned())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
    )
    .unwrap()
}

fn watch(
    root: &Path,
    args: &[&str],
    exercise: impl FnOnce(&mut TestTerminal, &Path) + Send + 'static,
) {
    let root = root.canonicalize().unwrap();
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_vt"));
    command.args(["run", "--watch"]);
    command.args(args);
    command.cwd(&root);
    command.env("PATH", binary_path());
    command.env("TERM", "xterm-256color");
    command.env("FORCE_COLOR", "1");
    let mut terminal = TestTerminal::spawn(ScreenSize { rows: 500, cols: 300 }, command).unwrap();
    let mut killer = terminal.child_handle.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            exercise(&mut terminal, &root);
            terminal.writer.write_all(b"\x03").unwrap();
            terminal.writer.flush().unwrap();
            let status = terminal.reader.wait_for_exit().unwrap();
            assert_eq!(status.exit_code(), 130, "{}", terminal.reader.screen_contents());
        }));
        if result.is_err() {
            let _ = terminal.writer.write_all(b"\x03");
            let _ = terminal.writer.flush();
        }
        let _ = tx.send(result);
    });
    match rx.recv_timeout(Duration::from_secs(45)) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let _ = killer.kill();
            std::panic::resume_unwind(error);
        }
        Err(error) => {
            let _ = killer.kill();
            panic!("watch test did not complete: {error}");
        }
    }
}

fn milestone(terminal: &mut TestTerminal, name: &str) {
    let _ = terminal.reader.expect_milestone(name);
}

fn wait_file(root: &Path, name: &str, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if std::fs::read_to_string(root.join(name)).is_ok_and(|value| value.contains(expected)) {
            return;
        }
        assert!(Instant::now() < deadline, "{name} never contained {expected}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn uncached_server_restarts_during_first_execution() {
    let root = fixture(
        &json!({"tasks": {"dev": {"command": "vtt watch-probe source.txt runs.txt server", "cache": false}}}),
    );
    write(root.path(), "source.txt", "one");
    watch(root.path(), &["dev"], |terminal, root| {
        milestone(terminal, "one");
        write(root, "source.txt", "two");
        milestone(terminal, "two");
        write(root, "source.txt", "three");
        milestone(terminal, "three");
        assert_eq!(std::fs::read_to_string(root.join("runs.txt")).unwrap(), "one\ntwo\nthree\n");
    });
}

#[test]
fn restart_reaps_descendants_before_reusing_their_port() {
    let port = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
    let root = fixture(
        &json!({"tasks": {"dev": {"command": format!("vtt watch-probe source.txt runs.txt tree {port}"), "cache": false}}}),
    );
    write(root.path(), "source.txt", "one");
    watch(root.path(), &["dev"], |terminal, root| {
        milestone(terminal, "one");
        write(root, "source.txt", "two");
        milestone(terminal, "two");
        assert_eq!(std::fs::read_to_string(root.join("runs.txt")).unwrap(), "one\ntwo\n");
    });
    std::net::TcpListener::bind(("127.0.0.1", port))
        .expect("Ctrl+C must release the descendant's port");
}

#[test]
fn failed_finite_task_waits_and_recovers_with_explicit_uncached_inputs() {
    let root = fixture(
        &json!({"tasks": {"build": {"command": "vtt watch-probe source.txt runs.txt", "cache": false, "input": ["source.txt"]}}}),
    );
    write(root.path(), "source.txt", "fail");
    watch(root.path(), &["build"], |terminal, root| {
        milestone(terminal, "fail");
        milestone(terminal, "watch-idle");
        write(root, "source.txt", "fixed");
        milestone(terminal, "fixed");
        milestone(terminal, "watch-idle");
        assert_eq!(std::fs::read_to_string(root.join("runs.txt")).unwrap(), "fail\nfixed\n");
    });
}

#[test]
fn cache_hit_keeps_inferred_inputs_watched() {
    let root = fixture(&json!({"tasks": {"build": "vtt watch-probe source.txt runs.txt"}}));
    write(root.path(), "source.txt", "one");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_vt"))
        .args(["run", "build"])
        .env("PATH", binary_path())
        .env("FORCE_COLOR", "1")
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    watch(root.path(), &["build"], |terminal, root| {
        milestone(terminal, "one");
        milestone(terminal, "watch-idle");
        assert!(
            terminal.reader.screen_contents().contains("cache hit"),
            "{}",
            terminal.reader.screen_contents()
        );
        write(root, "source.txt", "two");
        milestone(terminal, "two");
        milestone(terminal, "watch-idle");
        assert_eq!(std::fs::read_to_string(root.join("runs.txt")).unwrap(), "one\ntwo\n");
    });
}

#[test]
fn unrelated_server_survives_changes_and_atomic_replacements() {
    unrelated_server_case(false);
}

#[test]
fn nested_graph_keeps_unrelated_servers_running() {
    unrelated_server_case(true);
}

fn unrelated_server_case(nested: bool) {
    let root = fixture(&if nested { json!({"tasks":{"dev":"vt run -r dev"}}) } else { json!({}) });
    write(root.path(), "pnpm-workspace.yaml", "packages: ['packages/*']\n");
    for (package, value) in [("a", "a-one"), ("b", "b-one")] {
        write(
            root.path(),
            &format!("packages/{package}/package.json"),
            &json!({"name": package}).to_string(),
        );
        write(root.path(), &format!("packages/{package}/vite-task.json"), &json!({"tasks":{"dev":{"command":"vtt watch-probe source.txt runs.txt server", "cache":false}}}).to_string());
        write(root.path(), &format!("packages/{package}/source.txt"), value);
    }
    watch(root.path(), if nested { &["dev"] } else { &["-r", "dev"] }, |terminal, root| {
        wait_file(root, "packages/a/runs.txt", "a-one");
        wait_file(root, "packages/b/runs.txt", "b-one");
        write(root, "packages/a/replacement.txt", "a-two");
        std::fs::rename(
            root.join("packages/a/replacement.txt"),
            root.join("packages/a/source.txt"),
        )
        .unwrap();
        milestone(terminal, "a-two");
        std::fs::remove_file(root.join("packages/a/source.txt")).unwrap();
        milestone(terminal, "missing");
        write(root, "packages/a/source.txt", "a-three");
        milestone(terminal, "a-three");
        assert_eq!(std::fs::read_to_string(root.join("packages/b/runs.txt")).unwrap(), "b-one\n");
    });
}

#[test]
fn dependencies_rebuild_before_restarting_the_server() {
    let root = fixture(&json!({"tasks": {
        "build": {"command": "vtt cp source.txt artifact.txt", "input": ["source.txt"]},
        "dev": {"command": "vtt watch-probe artifact.txt runs.txt server", "cache": false, "dependsOn": ["build"]}
    }}));
    write(root.path(), "source.txt", "one");
    watch(root.path(), &["dev"], |terminal, root| {
        milestone(terminal, "one");
        write(root, "source.txt", "two");
        milestone(terminal, "two");
        write(root, "source.txt", "three");
        milestone(terminal, "three");
        assert_eq!(std::fs::read_to_string(root.join("runs.txt")).unwrap(), "one\ntwo\nthree\n");
    });
}

#[test]
fn cancelled_cached_servers_never_save_a_cache_hit() {
    let root = fixture(&json!({"tasks":{"dev":"vtt watch-probe source.txt runs.txt server"}}));
    write(root.path(), "source.txt", "one");
    for _ in 0..2 {
        watch(root.path(), &["dev"], |terminal, _| {
            milestone(terminal, "one");
            assert!(!terminal.reader.screen_contents().contains("cache hit"));
        });
    }
    assert_eq!(std::fs::read_to_string(root.path().join("runs.txt")).unwrap(), "one\none\n");
}

#[test]
fn directory_discovery_tracks_added_and_removed_entries() {
    let root = fixture(
        &json!({"tasks":{"list":{"command":"vtt watch-probe files runs.txt directory", "cache":false}}}),
    );
    std::fs::create_dir(root.path().join("files")).unwrap();
    watch(root.path(), &["list"], |terminal, root| {
        milestone(terminal, "empty");
        milestone(terminal, "watch-idle");
        write(root, "files/new", "contents");
        milestone(terminal, "new");
        milestone(terminal, "watch-idle");
        std::fs::remove_file(root.join("files/new")).unwrap();
        milestone(terminal, "empty");
        milestone(terminal, "watch-idle");
        assert_eq!(std::fs::read_to_string(root.join("runs.txt")).unwrap(), "empty\nnew\nempty\n");
    });
}

#[test]
fn command_sequence_restarts_from_its_first_item() {
    let root = fixture(
        &json!({"tasks":{"dev":{"command":["vtt cp source.txt artifact.txt", "vtt watch-probe artifact.txt runs.txt server"], "cache":false, "input":["source.txt"]}}}),
    );
    write(root.path(), "source.txt", "one");
    watch(root.path(), &["dev"], |terminal, root| {
        milestone(terminal, "one");
        write(root, "source.txt", "two");
        milestone(terminal, "two");
        assert_eq!(std::fs::read_to_string(root.join("artifact.txt")).unwrap(), "two");
    });
}

#[test]
fn no_cache_flag_retains_explicit_inputs() {
    let root = fixture(
        &json!({"tasks":{"dev":{"command":"vtt watch-probe source.txt runs.txt server", "input":["trigger.txt"]}}}),
    );
    write(root.path(), "source.txt", "one");
    write(root.path(), "trigger.txt", "start");
    watch(root.path(), &["--no-cache", "dev"], |terminal, root| {
        milestone(terminal, "one");
        write(root, "source.txt", "two");
        write(root, "trigger.txt", "restart");
        milestone(terminal, "two");
        assert_eq!(std::fs::read_to_string(root.join("runs.txt")).unwrap(), "one\ntwo\n");
    });
}

#[test]
fn watch_requires_a_valid_task_name() {
    let root = fixture(&json!({"tasks":{"build":"echo built"}}));
    for args in [vec!["run", "--watch"], vec!["run", "--watch", "unknown"]] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_vt"))
            .args(args)
            .current_dir(root.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("Watching task inputs"));
    }
}

#[test]
fn in_process_commands_keep_explicit_inputs() {
    let root = fixture(
        &json!({"tasks":{"build":{"command":"echo built", "cache":false, "input":["source.txt"]}}}),
    );
    write(root.path(), "source.txt", "one");
    watch(root.path(), &["build"], |terminal, root| {
        milestone(terminal, "watch-idle");
        write(root, "source.txt", "two");
        milestone(terminal, "watch-idle");
        assert!(terminal.reader.screen_contents().contains("Input changed; restarting"));
    });
}

#[test]
fn automatic_sequence_inputs_share_their_own_outputs() {
    let root = fixture(
        &json!({"tasks":{"dev":{"command":"vtt cp source.txt artifact.txt && vtt watch-probe artifact.txt runs.txt server", "cache":false}}}),
    );
    write(root.path(), "source.txt", "one");
    watch(root.path(), &["dev"], |terminal, root| {
        milestone(terminal, "one");
        write(root, "source.txt", "two");
        milestone(terminal, "two");
        write(root, "source.txt", "three");
        milestone(terminal, "three");
        assert_eq!(std::fs::read_to_string(root.join("runs.txt")).unwrap(), "one\ntwo\nthree\n");
    });
}

#[test]
fn nested_watch_remains_a_real_process_and_cleans_up_its_children() {
    let port = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
    let root = fixture(&json!({"tasks":{
        "outer":{"command":"vt run --watch inner", "input":["trigger.txt"]},
        "inner":{"command":format!("vtt watch-probe source.txt runs.txt tree {port}"), "cache":false}
    }}));
    write(root.path(), "source.txt", "one");
    write(root.path(), "trigger.txt", "start");
    watch(root.path(), &["outer"], |terminal, root| {
        milestone(terminal, "one");
        write(root, "source.txt", "two");
        milestone(terminal, "two");
        write(root, "trigger.txt", "restart outer");
        milestone(terminal, "two");
        assert_eq!(std::fs::read_to_string(root.join("runs.txt")).unwrap(), "one\ntwo\ntwo\n");
    });
    std::net::TcpListener::bind(("127.0.0.1", port))
        .expect("nested watch must release its descendant's port");
}

#[test]
fn changes_during_a_finite_run_never_cache_the_invalidated_result() {
    let root = fixture(
        &json!({"tasks":{"build":{"command":"vtt watch-probe source.txt runs.txt gate released", "input":["source.txt"]}}}),
    );
    write(root.path(), "source.txt", "one");
    watch(root.path(), &["build"], |terminal, root| {
        milestone(terminal, "one");
        write(root, "source.txt", "two");
        milestone(terminal, "two");
        write(root, "released", "");
        milestone(terminal, "watch-idle");
        write(root, "source.txt", "one");
        milestone(terminal, "one");
        milestone(terminal, "watch-idle");
        assert_eq!(std::fs::read_to_string(root.join("runs.txt")).unwrap(), "one\ntwo\none\n");
        assert!(!terminal.reader.screen_contents().contains("cache hit"));
    });
}

#[test]
fn concurrency_limit_serializes_independent_tasks() {
    let root = fixture(&json!({"tasks":{
        "a":{"command":"vtt watch-probe a.txt a-runs gate release-a", "cache":false,"input":["a.txt"]},
        "b":{"command":"vtt watch-probe b.txt b-runs gate release-b", "cache":false,"input":["b.txt"]},
        "all":{"command":"echo all", "dependsOn":["a","b"]}
    }}));
    write(root.path(), "a.txt", "a");
    write(root.path(), "b.txt", "b");
    watch(root.path(), &["--concurrency-limit", "1", "all"], |terminal, root| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !root.join("a-runs").exists() && !root.join("b-runs").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let (first, second) = if root.join("a-runs").exists() { ("a", "b") } else { ("b", "a") };
        milestone(terminal, first);
        assert!(!root.join(format!("{second}-runs")).exists());
        write(root, &format!("release-{first}"), "");
        milestone(terminal, second);
        write(root, &format!("release-{second}"), "");
        milestone(terminal, "watch-idle");
    });
}

#[test]
fn parallel_execution_retains_dependency_invalidation() {
    let root = fixture(&json!({"tasks":{
        "build":{"command":"vtt watch-probe source.txt builds gate released", "cache":false,"input":["source.txt"]},
        "dev":{"command":"vtt watch-probe server.txt runs.txt server", "cache":false,"dependsOn":["build"]}
    }}));
    write(root.path(), "source.txt", "build-one");
    write(root.path(), "server.txt", "server");
    watch(root.path(), &["--parallel", "dev"], |terminal, root| {
        wait_file(root, "builds", "build-one");
        milestone(terminal, "server");
        write(root, "source.txt", "build-two");
        milestone(terminal, "server");
        wait_file(root, "builds", "build-two");
        assert_eq!(std::fs::read_to_string(root.join("runs.txt")).unwrap(), "server\nserver\n");
    });
}

#[test]
fn burst_saves_are_coalesced_and_shared_inputs_restart_every_consumer() {
    let root = fixture(&json!({"tasks":{
        "a":{"command":"vtt watch-probe source.txt a-runs server", "cache":false},
        "b":{"command":"vtt watch-probe source.txt b-runs server", "cache":false},
        "all":{"command":"echo all", "dependsOn":["a","b"]}
    }}));
    write(root.path(), "source.txt", "one");
    watch(root.path(), &["--parallel", "all"], |terminal, root| {
        wait_file(root, "a-runs", "one");
        wait_file(root, "b-runs", "one");
        write(root, "source.txt", "two");
        write(root, "source.txt", "three");
        wait_file(root, "a-runs", "three");
        wait_file(root, "b-runs", "three");
        milestone(terminal, "three");
        assert_eq!(std::fs::read_to_string(root.join("a-runs")).unwrap(), "one\nthree\n");
        assert_eq!(std::fs::read_to_string(root.join("b-runs")).unwrap(), "one\nthree\n");
    });
}

#[test]
fn script_hooks_restart_with_their_task() {
    let root = fixture(&json!({"cache":false}));
    write(
        root.path(),
        "package.json",
        &json!({"name":"watch-test","scripts":{
            "prebuild":"vtt watch-probe pre.txt pre-runs",
            "build":"vtt watch-probe source.txt runs.txt",
            "postbuild":"vtt watch-probe post.txt post-runs server"
        }})
        .to_string(),
    );
    write(root.path(), "pre.txt", "pre");
    write(root.path(), "source.txt", "one");
    write(root.path(), "post.txt", "post");
    watch(root.path(), &["build"], |terminal, root| {
        milestone(terminal, "post");
        write(root, "source.txt", "two");
        milestone(terminal, "post");
        assert_eq!(std::fs::read_to_string(root.join("pre-runs")).unwrap(), "pre\npre\n");
        assert_eq!(std::fs::read_to_string(root.join("runs.txt")).unwrap(), "one\ntwo\n");
        assert_eq!(std::fs::read_to_string(root.join("post-runs")).unwrap(), "post\npost\n");
    });
}

#[test]
fn new_files_matching_explicit_globs_restart_uncached_servers() {
    let root = fixture(
        &json!({"tasks":{"dev":{"command":"vtt watch-probe source.txt runs.txt server", "cache":false,"input":["extra/**/*.trigger"]}}}),
    );
    write(root.path(), "source.txt", "one");
    watch(root.path(), &["dev"], |terminal, root| {
        milestone(terminal, "one");
        write(root, "source.txt", "two");
        write(root, "extra/new/file.trigger", "created");
        milestone(terminal, "two");
        assert_eq!(std::fs::read_to_string(root.join("runs.txt")).unwrap(), "one\ntwo\n");
    });
}

#[test]
fn every_log_mode_forwards_output_across_generations() {
    for mode in ["interleaved", "labeled", "grouped"] {
        let root = fixture(
            &json!({"tasks":{"build":{"command":"vtt watch-probe source.txt runs.txt", "cache":false}}}),
        );
        write(root.path(), "source.txt", "one");
        watch(root.path(), &["--log", mode, "build"], |terminal, root| {
            milestone(terminal, "watch-idle");
            write(root, "source.txt", "two");
            milestone(terminal, "watch-idle");
            let screen = terminal.reader.screen_contents();
            assert!(screen.contains("input=one") && screen.contains("input=two"), "{screen}");
        });
    }
}
