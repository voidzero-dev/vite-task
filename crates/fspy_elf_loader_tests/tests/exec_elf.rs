// exec_elf implements the Linux process ABI. Its execution tests need Linux,
// even though Cargo can cross-compile the freestanding artifact on other hosts.
#![cfg(target_os = "linux")]

use std::{
    ffi::OsStr,
    mem::{offset_of, size_of},
    os::unix::{
        ffi::OsStrExt as _,
        fs as unix_fs,
        process::{CommandExt as _, ExitStatusExt as _},
    },
    process::{Command, Output, Stdio},
};

use ntest::timeout;

const LAUNCHER: &str = env!("CARGO_BIN_FILE_FSPY_ELF_LOADER_TEST_LAUNCHER");
const FIXTURE_DYN_PIE: &str = env!("FIXTURE_DYN_PIE");
const FIXTURE_DYN_EXEC: &str = env!("FIXTURE_DYN_EXEC");
const FIXTURE_STATIC_EXEC: &str = env!("FIXTURE_STATIC_EXEC");
const FIXTURE_STATIC_PIE: &str = env!("FIXTURE_STATIC_PIE");
const FIXTURE_VIRT_DYN_PIE: &str = env!("FIXTURE_VIRT_DYN_PIE");
const FIXTURE_VIRT_STATIC_EXEC: &str = env!("FIXTURE_VIRT_STATIC_EXEC");

#[test]
#[timeout(10_000)]
fn echo_matches_kernel_exec() {
    let mut direct = Command::new("/bin/echo");
    direct.args(["hello", "world"]);
    let mut loaded = launcher();
    // BusyBox selects its applet through argv[0]. The runtime fixtures below
    // exercise custom argv[0] independently of the host's echo implementation.
    loaded.args(["/bin/echo", "echo", "hello", "world"]);
    let direct = output(direct);
    let loaded = output(loaded);
    assert_eq!(loaded.status.code(), Some(0), "{loaded:?}");
    assert_eq!(loaded.stdout, b"hello world\n");
    assert_eq!(loaded.stdout, direct.stdout);
    assert_eq!(loaded.stderr, direct.stderr);
}

#[test]
#[timeout(10_000)]
fn initializes_dynamic_pie_runtime() {
    check_runtime_image(FIXTURE_DYN_PIE, elf::abi::ET_DYN, true);
}

#[test]
#[timeout(10_000)]
fn initializes_fixed_runtime() {
    check_runtime_image(FIXTURE_DYN_EXEC, elf::abi::ET_EXEC, true);
}

fn configure_runtime_fixture(
    command: &mut Command,
    fixture: &str,
    interpreter: bool,
    fixed_address: bool,
) {
    command.arg("").arg(OsStr::from_bytes(b"\xff"));
    command.env_clear().env("FIXTURE_IMAGE", fixture).env("EMPTY", "");
    command.env("BYTES", OsStr::from_bytes(b"\xff"));
    if interpreter {
        command.env("EXPECT_INTERPRETER", "1");
    }
    if fixed_address {
        command.env("EXPECT_FIXED_ADDRESS", "1");
    }
}

fn run_loaded_fixture(fixture: &str, interpreter: bool, fixed_address: bool) -> Output {
    let mut command = launcher();
    command.args([fixture, "custom-argv-zero"]);
    configure_runtime_fixture(&mut command, fixture, interpreter, fixed_address);
    output(command)
}

fn check_runtime_image(fixture: &str, image_type: u16, interpreter: bool) {
    assert_image(fixture, image_type, interpreter);
    let fixed_address = image_type == elf::abi::ET_EXEC;

    // Run each fixture through kernel execve first so fixture/toolchain issues
    // are distinguished from loader regressions.
    let mut direct = Command::new(fixture);
    direct.arg0("custom-argv-zero").stderr(Stdio::piped());
    configure_runtime_fixture(&mut direct, fixture, interpreter, fixed_address);
    let direct = output(direct);
    assert_eq!(direct.status.code(), Some(37), "kernel exec: {direct:?}");
    assert_eq!(direct.stdout, b"runtime-ok\n");
    assert!(direct.stderr.is_empty(), "kernel exec: {direct:?}");

    let loaded = run_loaded_fixture(fixture, interpreter, fixed_address);
    assert_eq!(loaded.status.code(), Some(37), "loader exec: {loaded:?}");
    assert_eq!(loaded.stdout, direct.stdout);
    assert_eq!(loaded.stderr, direct.stderr);
}

#[test]
#[timeout(10_000)]
fn initializes_static_runtime() {
    check_runtime_image(FIXTURE_STATIC_EXEC, elf::abi::ET_EXEC, false);
}

#[test]
#[timeout(10_000)]
fn initializes_static_pie_runtime() {
    check_runtime_image(FIXTURE_STATIC_PIE, elf::abi::ET_DYN, false);
}

#[test]
#[timeout(10_000)]
fn rejects_fixed_image_overlapping_launcher() {
    let launcher_bytes = std::fs::read(LAUNCHER).unwrap();
    let launcher_elf =
        elf::ElfBytes::<elf::endian::LittleEndian>::minimal_parse(&launcher_bytes).unwrap();
    let launcher_base = launcher_elf
        .segments()
        .unwrap()
        .iter()
        .find(|ph| ph.p_type == elf::abi::PT_LOAD)
        .unwrap()
        .p_vaddr;
    let mut bytes = std::fs::read(FIXTURE_DYN_EXEC).unwrap();
    let parsed = elf::ElfBytes::<elf::endian::LittleEndian>::minimal_parse(&bytes).unwrap();
    let table = usize::try_from(parsed.ehdr.e_phoff).unwrap();
    let loads: Vec<_> = parsed
        .segments()
        .unwrap()
        .iter()
        .enumerate()
        .filter(|(_, ph)| ph.p_type == elf::abi::PT_LOAD)
        .collect();
    let base = loads[0].1.p_vaddr;
    let entry = parsed.ehdr.e_entry - base + launcher_base;
    write_u64(&mut bytes, offset_of!(libc::Elf64_Ehdr, e_entry), entry);
    for (index, ph) in loads {
        let phdr_offset = table + index * size_of::<libc::Elf64_Phdr>();
        write_u64(
            &mut bytes,
            phdr_offset + offset_of!(libc::Elf64_Phdr, p_vaddr),
            ph.p_vaddr - base + launcher_base,
        );
    }
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    let mut command = launcher();
    command.arg(file.path()).arg("overlapping-image");
    let output = output(command);
    assert_eq!(output.status.code(), Some(101), "{output:?}");
    assert!(
        str::from_utf8(&output.stderr).unwrap().contains("cannot reserve fixed ELF address range"),
        "{output:?}"
    );
}

#[test]
#[ignore = "requires Node.js"]
#[timeout(10_000)]
fn node_runs_javascript() {
    // Resolve the actual executable even when PATH contains a Node wrapper.
    let mut resolve = Command::new("node");
    resolve.args(["-p", "process.execPath"]);
    let resolved = output(resolve);
    assert!(resolved.status.success(), "{resolved:?}");
    let node = OsStr::from_bytes(resolved.stdout.strip_suffix(b"\n").unwrap());
    let script =
        "console.log(require('node:crypto').createHash('sha256').update('loader').digest('hex'))";
    let mut direct = Command::new(node);
    direct.args(["-e", script]);
    let mut loaded = launcher();
    loaded.arg(node).args(["node", "-e", script]);
    let direct = output(direct);
    let loaded = output(loaded);
    assert_eq!(direct.status.code(), Some(0), "{direct:?}");
    assert_eq!(loaded.status.code(), Some(0), "{loaded:?}");
    assert_eq!(loaded.stdout, direct.stdout);
    assert_eq!(loaded.stderr, direct.stderr);
}

#[test]
#[ignore = "requires Node.js"]
#[timeout(10_000)]
fn node_proc_self_exe_and_exec_path_match_direct_exec() {
    let mut resolve = Command::new("node");
    resolve.args(["-p", "process.execPath"]);
    let resolved = output(resolve);
    assert!(resolved.status.success(), "{resolved:?}");
    let node = OsStr::from_bytes(resolved.stdout.strip_suffix(b"\n").unwrap());
    let script = "const fs = require('node:fs'); console.log(JSON.stringify([process.execPath, fs.readlinkSync('/proc/self/exe'), fs.realpathSync('/proc/self/exe')]));";
    let mut direct = Command::new(node);
    direct.args(["-e", script]);
    let mut loaded = launcher();
    loaded.arg(node).args(["node", "-e", script]);
    let direct = output(direct);
    let loaded = output(loaded);
    assert_eq!(direct.status.code(), Some(0), "{direct:?}");
    assert_eq!(loaded.status.code(), Some(0), "{loaded:?}");
    assert_eq!(loaded.stdout, direct.stdout);
    assert_eq!(loaded.stderr, direct.stderr);
}

#[test]
#[timeout(10_000)]
fn virtualizes_sigsys_sigaction_and_forwarding() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "sigaction", b"sigaction-ok\n");
    }
}

#[test]
#[timeout(10_000)]
fn non_seccomp_sigsys_with_sig_dfl_terminates_process() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        let mut direct = Command::new(fixture);
        direct.arg("sigsys-default-death").stderr(Stdio::piped());
        let direct = output(direct);
        assert_eq!(direct.status.signal(), Some(libc::SIGSYS), "kernel exec: {direct:?}");

        let mut loaded = launcher();
        loaded.args([fixture, fixture, "sigsys-default-death"]);
        let loaded = output(loaded);
        assert_eq!(loaded.status.signal(), Some(libc::SIGSYS), "loader exec: {loaded:?}");
    }
}

#[test]
#[timeout(10_000)]
fn virtualizes_sigsys_blocking_single_threaded() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "sigmask-single", b"sigmask-single-ok\n");
    }
}

#[test]
#[timeout(10_000)]
fn virtualizes_sigsys_blocking_multi_threaded() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "sigmask-threads", b"sigmask-threads-ok\n");
    }
}

#[test]
#[timeout(10_000)]
fn virtualizes_proc_self_exe_syscalls() {
    let tmp = tempfile::tempdir().unwrap();
    let other_exe_link = tmp.path().join("exe");
    let other_exe_subdir = tmp.path().join("sub");
    std::fs::create_dir(&other_exe_subdir).unwrap();
    unix_fs::symlink("/dev/null", &other_exe_link).unwrap();

    // Spawn a separate process so `/proc/<other_pid>/exe` non-rewrite is tested
    // deterministically with matching UID.
    let mut sleeper = Command::new("/bin/sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let other_pid = sleeper.id().to_string();
    let other_pid_exe = std::fs::canonicalize("/bin/sleep").unwrap();

    for (sym_name, fixture) in [
        ("sym_virt_dyn_pie", FIXTURE_VIRT_DYN_PIE),
        ("sym_virt_static_exec", FIXTURE_VIRT_STATIC_EXEC),
    ] {
        let canonical = std::fs::canonicalize(fixture).unwrap();

        // Direct path launch.
        let mut direct = Command::new(fixture);
        direct
            .arg("proc-exe")
            .env("EXPECTED_EXE", &canonical)
            .env("OTHER_EXE_LINK", &other_exe_link)
            .env("OTHER_EXE_DIR", tmp.path())
            .env("OTHER_EXE_SUBDIR", &other_exe_subdir)
            .env("OTHER_PID", &other_pid)
            .env("OTHER_PID_EXE", &other_pid_exe)
            .stderr(Stdio::piped());
        let direct = output(direct);
        assert_eq!(direct.status.code(), Some(0), "kernel exec ({fixture}): {direct:?}");
        assert_eq!(direct.stdout, b"proc-exe-ok\n");

        let mut loaded = launcher();
        loaded
            .args([fixture, "custom-argv0", "proc-exe"])
            .env("EXPECTED_EXE", &canonical)
            .env("OTHER_EXE_LINK", &other_exe_link)
            .env("OTHER_EXE_DIR", tmp.path())
            .env("OTHER_EXE_SUBDIR", &other_exe_subdir)
            .env("OTHER_PID", &other_pid)
            .env("OTHER_PID_EXE", &other_pid_exe);
        let loaded = output(loaded);
        assert_eq!(loaded.status.code(), Some(0), "loader exec ({fixture}): {loaded:?}");
        assert_eq!(loaded.stdout, direct.stdout);
        assert_eq!(loaded.stderr, direct.stderr);

        // Launch via symlink: /proc/self/exe must resolve to the canonical target.
        let sym_target = tmp.path().join(sym_name);
        unix_fs::symlink(fixture, &sym_target).unwrap();
        let mut via_symlink = launcher();
        via_symlink
            .arg(&sym_target)
            .args(["sym-argv0", "proc-exe"])
            .env("EXPECTED_EXE", &canonical)
            .env("OTHER_EXE_LINK", &other_exe_link)
            .env("OTHER_EXE_DIR", tmp.path())
            .env("OTHER_EXE_SUBDIR", &other_exe_subdir)
            .env("OTHER_PID", &other_pid)
            .env("OTHER_PID_EXE", &other_pid_exe);
        let via_symlink = output(via_symlink);
        assert_eq!(
            via_symlink.status.code(),
            Some(0),
            "symlink loader ({fixture}): {via_symlink:?}"
        );
        assert_eq!(via_symlink.stdout, b"proc-exe-ok\n");
        assert!(via_symlink.stderr.is_empty(), "{via_symlink:?}");
    }

    let _ = sleeper.kill();
    let _ = sleeper.wait();
}

#[test]
#[timeout(10_000)]
fn sets_proc_self_comm_from_execfn_basename() {
    let tmp = tempfile::tempdir().unwrap();
    // Both a short basename and a >15-byte basename (truncated to 15 bytes).
    for (link_name, expected_comm) in [
        ("short_target", "short_target"),
        ("very_long_executable_name_exceeding_15", "very_long_execu"),
    ] {
        let sym = tmp.path().join(link_name);
        unix_fs::symlink(FIXTURE_VIRT_DYN_PIE, &sym).unwrap();

        let mut loaded = launcher();
        loaded.arg(&sym).args(["different-argv0", "comm"]).env("EXPECTED_COMM", expected_comm);
        let loaded = output(loaded);
        assert_eq!(loaded.status.code(), Some(0), "{link_name}: {loaded:?}");
        assert_eq!(loaded.stdout, b"comm-ok\n");
        assert!(loaded.stderr.is_empty(), "{link_name}: {loaded:?}");
    }
}

#[test]
#[timeout(10_000)]
fn respects_rlimit_stack_above_default_8_mib() {
    let mut command = launcher();
    command.args([FIXTURE_VIRT_DYN_PIE, "deep-stack-argv0", "deep-stack"]);
    // SAFETY: `setrlimit` is async-signal-safe and runs in the forked child
    // before `execve` of the launcher.
    unsafe {
        command.pre_exec(|| {
            let mut rlim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            if libc::getrlimit(libc::RLIMIT_STACK, &raw mut rlim) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let target = 16 * 1024 * 1024;
            if rlim.rlim_max != libc::RLIM_INFINITY && rlim.rlim_max < target {
                rlim.rlim_cur = rlim.rlim_max;
            } else {
                rlim.rlim_cur = target;
            }
            if libc::setrlimit(libc::RLIMIT_STACK, &raw const rlim) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let out = output(command);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(out.stdout, b"deep-stack-ok\n");
    assert!(out.stderr.is_empty(), "{out:?}");
}

#[test]
#[timeout(10_000)]
fn syscall_interceptor_replays_short_circuits_and_observes_proc_exe_rewrite() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        let canonical = std::fs::canonicalize(fixture).unwrap();
        let mut loaded = launcher();
        loaded
            .args(["--test-interceptor", fixture, "interceptor-argv0", "interceptor"])
            .env("EXPECTED_EXE", &canonical);
        let loaded = output(loaded);
        assert_eq!(loaded.status.code(), Some(0), "interceptor ({fixture}): {loaded:?}");
        assert_eq!(loaded.stdout, b"interceptor-ok\n");
        assert!(loaded.stderr.is_empty(), "interceptor ({fixture}): {loaded:?}");
    }
}

#[test]
#[timeout(10_000)]
fn recursive_re_exec_does_not_stack_seccomp_filters() {
    for use_interceptor in [false, true] {
        for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
            let canonical = std::fs::canonicalize(fixture).unwrap();
            let mut loaded = launcher();
            if use_interceptor {
                loaded.arg("--test-interceptor");
            }
            loaded
                .args([fixture, "recursive-stage-1", "recursive-exec"])
                .env("EXPECTED_EXE", &canonical)
                .env("LAUNCHER_PATH", LAUNCHER)
                .env("RECURSIVE_STAGE", "1")
                .env("USE_INTERCEPTOR", if use_interceptor { "1" } else { "0" });
            let loaded = output(loaded);
            assert_eq!(
                loaded.status.code(),
                Some(0),
                "recursive ({fixture}, interceptor={use_interceptor}): {loaded:?}"
            );
            assert_eq!(loaded.stdout, b"recursive-exec-ok\n");
            assert!(loaded.stderr.is_empty(), "{loaded:?}");
        }
    }
}

#[test]
#[timeout(10_000)]
fn forwards_target_installed_seccomp_ret_trap_to_target_sigsys_handler() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "nested-seccomp", b"nested-seccomp-ok\n");
    }
}

#[test]
#[timeout(10_000)]
fn posix_spawn_survives_clone3_clear_sighand() {
    // Exercise glibc's `posix_spawn` (which tries `clone3(CLONE_CLEAR_SIGHAND)`
    // and must fall back cleanly to `clone(2)` without resetting `SIGSYS`).
    let mut direct = Command::new(FIXTURE_VIRT_DYN_PIE);
    direct.arg("posix-spawn").env_remove("LAUNCHER_PATH").stderr(Stdio::piped());
    let direct = output(direct);
    assert_eq!(direct.status.code(), Some(0), "kernel exec: {direct:?}");
    assert_eq!(direct.stdout, b"posix-spawn-ok\n");
    assert!(direct.stderr.is_empty(), "kernel exec: {direct:?}");

    let mut loaded = launcher();
    loaded
        .args([FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_DYN_PIE, "posix-spawn"])
        .env("LAUNCHER_PATH", LAUNCHER);
    let loaded = output(loaded);
    assert_eq!(loaded.status.code(), Some(0), "loader exec: {loaded:?}");
    assert_eq!(loaded.stdout, direct.stdout);
    assert_eq!(loaded.stderr, direct.stderr);

    // Both dynamic and static binaries observe `ENOSYS` from `clone3`.
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        let mut loaded = launcher();
        loaded.args([fixture, fixture, "clone3-enosys"]);
        let loaded = output(loaded);
        assert_eq!(loaded.status.code(), Some(0), "clone3-enosys ({fixture}): {loaded:?}");
        assert_eq!(loaded.stdout, b"clone3-enosys-ok\n");
        assert!(loaded.stderr.is_empty(), "clone3-enosys ({fixture}): {loaded:?}");
    }
}

#[test]
#[timeout(10_000)]
fn survives_sa_onstack_async_signal_storm_during_trapped_syscalls() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "sigaltstack-storm", b"sigaltstack-storm-ok\n");
    }
}

#[test]
#[timeout(10_000)]
fn blocking_fifo_open_is_interruptible_by_signal() {
    let tmp = tempfile::tempdir().unwrap();
    for (name, fixture) in
        [("test_0.fifo", FIXTURE_VIRT_DYN_PIE), ("test_1.fifo", FIXTURE_VIRT_STATIC_EXEC)]
    {
        let fifo = tmp.path().join(name);
        let mut loaded = launcher();
        loaded.args([fixture, fixture, "fifo-open-eintr"]).env("FIFO_PATH", &fifo);
        let loaded = output(loaded);
        assert_eq!(loaded.status.code(), Some(0), "fifo-open-eintr ({fixture}): {loaded:?}");
        assert_eq!(loaded.stdout, b"fifo-open-eintr-ok\n");
        assert!(loaded.stderr.is_empty(), "fifo-open-eintr ({fixture}): {loaded:?}");
    }
}

#[test]
#[timeout(10_000)]
fn blocked_sigsys_stays_pending_until_unblocked() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "blocked-sigsys-pending", b"blocked-sigsys-pending-ok\n");
    }
}

#[test]
#[timeout(10_000)]
fn masked_wait_honors_sigsys_deferral_and_delivery() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "wait-mask-sigsys", b"wait-mask-sigsys-ok\n");
    }
}

#[test]
#[timeout(30_000)]
fn dead_tid_reaping_handles_churn_past_table_capacity() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "tid-churn", b"tid-churn-ok\n");
    }
}

#[test]
#[timeout(15_000)]
fn fork_inherits_virtual_sigmask_and_resets_action_lock() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "fork-sigmask-and-lock", b"fork-sigmask-and-lock-ok\n");
    }
}

#[test]
#[timeout(10_000)]
fn vfork_posix_spawn_does_not_clobber_parent_sigsys_state() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        let canonical = std::fs::canonicalize(fixture).unwrap();

        let mut direct = Command::new(fixture);
        direct
            .arg("vfork-parent-isolation")
            .env("EXPECTED_EXE", &canonical)
            .env_remove("LAUNCHER_PATH")
            .stderr(Stdio::piped());
        let direct = output(direct);
        assert_eq!(direct.status.code(), Some(0), "kernel exec ({fixture}): {direct:?}");
        assert_eq!(direct.stdout, b"vfork-parent-isolation-ok\n");
        assert!(direct.stderr.is_empty(), "kernel exec ({fixture}): {direct:?}");

        let mut loaded = launcher();
        loaded
            .args([fixture, fixture, "vfork-parent-isolation"])
            .env("EXPECTED_EXE", &canonical)
            .env("LAUNCHER_PATH", LAUNCHER);
        let loaded = output(loaded);
        assert_eq!(loaded.status.code(), Some(0), "loader exec ({fixture}): {loaded:?}");
        assert_eq!(loaded.stdout, direct.stdout);
        assert_eq!(loaded.stderr, direct.stderr);
    }
}

#[test]
#[timeout(10_000)]
fn tiny_5kib_vfork_stack_with_guard_page_survives_trapped_syscalls() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "tiny-vfork-stack", b"tiny-vfork-stack-ok\n");
    }
}

#[test]
#[timeout(10_000)]
fn abseil_address_probe_reaches_the_kernel() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "abseil-address-probe", b"abseil-address-probe-ok\n");
    }
}

#[test]
#[timeout(15_000)]
fn vfork_shared_stack_deep_frame_preserves_parent_regs() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "vfork-shared-stack-deep", b"vfork-shared-stack-deep-ok\n");
    }
}

#[test]
#[timeout(10_000)]
fn clone_new_mm_with_custom_stack_updates_owner_pid() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(
            fixture,
            "clone-custom-stack-new-mm",
            b"clone-custom-stack-new-mm-ok\n",
        );
    }
}

#[test]
#[timeout(10_000)]
fn pselect_deferred_sigsys_does_not_restart_timeout() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(
            fixture,
            "pselect-timeout-deferred-sigsys",
            b"pselect-timeout-deferred-sigsys-ok\n",
        );
    }
}

#[test]
#[timeout(10_000)]
fn proc_sibling_tid_exe_is_virtualized() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "proc-sibling-tid-exe", b"proc-sibling-tid-exe-ok\n");
    }
}

#[test]
#[timeout(20_000)]
fn altstack_8kib_signal_storm_with_trapped_syscalls() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "altstack-8kib-storm", b"altstack-8kib-storm-ok\n");
    }
}

#[test]
#[timeout(10_000)]
fn pselect_simultaneous_eintr_and_fdset_clear() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(
            fixture,
            "pselect-simultaneous-eintr",
            b"pselect-simultaneous-eintr-ok\n",
        );
    }
}

#[test]
#[timeout(10_000)]
fn posix_spawnp_long_path_preserves_small_child_stack() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "posix-spawnp-long-path", b"posix-spawnp-long-path-ok\n");
    }
}

#[test]
#[timeout(10_000)]
fn io_uring_setup_returns_enosys() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        let mut loaded = launcher();
        loaded.args([fixture, fixture, "io-uring-enosys"]);
        let loaded = output(loaded);
        assert_eq!(loaded.status.code(), Some(0), "io-uring-enosys ({fixture}): {loaded:?}");
        assert_eq!(loaded.stdout, b"io-uring-enosys-ok\n");
        assert!(loaded.stderr.is_empty(), "io-uring-enosys ({fixture}): {loaded:?}");
    }
}

#[test]
#[timeout(10_000)]
fn mismatched_fingerprint_re_exec_installs_new_filter() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        let canonical = std::fs::canonicalize(fixture).unwrap();
        let mut loaded = launcher();
        // Stage 1 runs without `--test-interceptor` and re-execs with `--test-interceptor`.
        loaded
            .args([fixture, "recursive-stage-1", "recursive-exec"])
            .env("EXPECTED_EXE", &canonical)
            .env("LAUNCHER_PATH", LAUNCHER)
            .env("RECURSIVE_STAGE", "1")
            .env("USE_INTERCEPTOR", "1")
            .env("EXPECT_FILTER_INCREMENT", "1");
        let loaded = output(loaded);
        assert_eq!(loaded.status.code(), Some(0), "mismatched-fp ({fixture}): {loaded:?}");
        assert_eq!(loaded.stdout, b"recursive-exec-ok\n");
        assert!(loaded.stderr.is_empty(), "{loaded:?}");
    }
}

#[test]
#[timeout(10_000)]
fn inherits_pre_exec_sigsys_ign_and_blocked_mask() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        let canonical = std::fs::canonicalize(fixture).unwrap();
        let configure_pre_exec = |cmd: &mut Command| {
            // SAFETY: `sigaction` and `sigprocmask` are async-signal-safe and
            // run in the forked child prior to `execve`.
            unsafe {
                cmd.pre_exec(|| {
                    let mut sa: libc::sigaction = std::mem::zeroed();
                    sa.sa_sigaction = libc::SIG_IGN;
                    if libc::sigaction(libc::SIGSYS, &raw const sa, std::ptr::null_mut()) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let mut set: libc::sigset_t = std::mem::zeroed();
                    libc::sigemptyset(&raw mut set);
                    libc::sigaddset(&raw mut set, libc::SIGSYS);
                    libc::sigaddset(&raw mut set, libc::SIGUSR1);
                    if libc::sigprocmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut()) != 0
                    {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        };

        let mut direct = Command::new(fixture);
        direct.arg("inherited-sigsys").env("EXPECTED_EXE", &canonical).stderr(Stdio::piped());
        configure_pre_exec(&mut direct);
        let direct = output(direct);
        assert_eq!(direct.status.code(), Some(0), "kernel exec ({fixture}): {direct:?}");
        assert_eq!(direct.stdout, b"inherited-sigsys-ok\n");

        let mut loaded = launcher();
        loaded.args([fixture, fixture, "inherited-sigsys"]).env("EXPECTED_EXE", &canonical);
        configure_pre_exec(&mut loaded);
        let loaded = output(loaded);
        assert_eq!(loaded.status.code(), Some(0), "loader exec ({fixture}): {loaded:?}");
        assert_eq!(loaded.stdout, direct.stdout);
        assert_eq!(loaded.stderr, direct.stderr);
    }
}

#[test]
#[timeout(10_000)]
fn aarch64_x28_survives_masked_wait_open_and_forwarded_sigsys() {
    let tmp = tempfile::tempdir().unwrap();
    for (direct_name, loaded_name, fixture) in [
        ("x28_direct_0.fifo", "x28_loaded_0.fifo", FIXTURE_VIRT_DYN_PIE),
        ("x28_direct_1.fifo", "x28_loaded_1.fifo", FIXTURE_VIRT_STATIC_EXEC),
    ] {
        let fifo_direct = tmp.path().join(direct_name);
        let mut direct = Command::new(fixture);
        direct.arg("aarch64-x28").env("FIFO_PATH", &fifo_direct).stderr(Stdio::piped());
        let direct = output(direct);
        assert_eq!(direct.status.code(), Some(0), "kernel exec ({fixture}): {direct:?}");
        assert_eq!(direct.stdout, b"aarch64-x28-ok\n");

        let fifo_loaded = tmp.path().join(loaded_name);
        let mut loaded = launcher();
        loaded.args([fixture, fixture, "aarch64-x28"]).env("FIFO_PATH", &fifo_loaded);
        let loaded = output(loaded);
        assert_eq!(loaded.status.code(), Some(0), "loader exec ({fixture}): {loaded:?}");
        assert_eq!(loaded.stdout, direct.stdout);
        assert_eq!(loaded.stderr, direct.stderr);
    }
}

#[test]
#[timeout(15_000)]
fn vfork_current_sp_and_separate_altstack_preserve_state() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "vfork-altstack-save", b"vfork-altstack-save-ok\n");
    }
}

#[test]
#[timeout(20_000)]
fn stack_pool_overflow_handles_more_than_64_concurrent_threads() {
    for fixture in [FIXTURE_VIRT_DYN_PIE, FIXTURE_VIRT_STATIC_EXEC] {
        check_virt_subcommand(fixture, "stack-pool-overflow", b"stack-pool-overflow-ok\n");
    }
}

fn check_virt_subcommand(fixture: &str, subcommand: &str, expected_stdout: &[u8]) {
    let canonical = std::fs::canonicalize(fixture).unwrap();

    let mut direct = Command::new(fixture);
    direct.arg(subcommand).env("EXPECTED_EXE", &canonical).stderr(Stdio::piped());
    let direct = output(direct);
    assert_eq!(direct.status.code(), Some(0), "kernel exec ({fixture} {subcommand}): {direct:?}");
    assert_eq!(direct.stdout, expected_stdout);
    assert!(direct.stderr.is_empty(), "kernel exec ({fixture} {subcommand}): {direct:?}");

    let mut loaded = launcher();
    loaded
        .args([fixture, fixture, subcommand])
        .env("EXPECTED_EXE", &canonical)
        .env("FSPY_LOADER_EXEC", "1")
        .env("LAUNCHER_PATH", LAUNCHER);
    let loaded = output(loaded);
    assert_eq!(loaded.status.code(), Some(0), "loader exec ({fixture} {subcommand}): {loaded:?}");
    assert_eq!(loaded.stdout, direct.stdout);
    assert_eq!(loaded.stderr, direct.stderr);
}

#[test]
#[timeout(10_000)]
fn rejects_malformed_images() {
    let base = std::fs::read(FIXTURE_DYN_PIE).unwrap();
    let mut overflow = base.clone();
    write_u64(&mut overflow, offset_of!(libc::Elf64_Ehdr, e_phoff), u64::MAX);
    let mut wrong_machine = base.clone();
    write_u16(&mut wrong_machine, offset_of!(libc::Elf64_Ehdr, e_machine), 0);
    let mut unsupported_type = base.clone();
    write_u16(&mut unsupported_type, offset_of!(libc::Elf64_Ehdr, e_type), elf::abi::ET_REL);
    let parsed = elf::ElfBytes::<elf::endian::LittleEndian>::minimal_parse(&base).unwrap();
    let load =
        parsed.segments().unwrap().iter().position(|ph| ph.p_type == elf::abi::PT_LOAD).unwrap();
    let phdr = usize::try_from(parsed.ehdr.e_phoff).unwrap() + load * size_of::<libc::Elf64_Phdr>();
    let mut invalid_size = base.clone();
    write_u64(&mut invalid_size, phdr + offset_of!(libc::Elf64_Phdr, p_filesz), u64::MAX);
    for (case, bytes) in [
        ("truncated header", b"not ELF".to_vec()),
        ("truncated program headers", base[..size_of::<libc::Elf64_Ehdr>()].to_vec()),
        ("overflowing program-header offset", overflow),
        ("wrong architecture", wrong_machine),
        ("unsupported type", unsupported_type),
        ("invalid size", invalid_size),
    ] {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), bytes).unwrap();
        let mut command = launcher();
        command.arg(file.path()).arg("invalid-image");
        let output = output(command);
        assert_eq!(output.status.code(), Some(101), "{case}: {output:?}");
        assert!(
            str::from_utf8(&output.stderr).unwrap().contains("panicked at"),
            "{case}: {output:?}"
        );
    }
}

#[test]
#[timeout(10_000)]
fn runs_images_with_an_executable_stack() {
    // Like Linux, an executable PT_GNU_STACK maps the stack executable rather
    // than rejecting the image (e.g. objects built without `.note.GNU-stack`).
    let mut bytes = std::fs::read(FIXTURE_DYN_PIE).unwrap();
    let parsed = elf::ElfBytes::<elf::endian::LittleEndian>::minimal_parse(&bytes).unwrap();
    let stack = parsed
        .segments()
        .unwrap()
        .iter()
        .position(|ph| ph.p_type == elf::abi::PT_GNU_STACK)
        .expect("fixture has PT_GNU_STACK");
    let phdr =
        usize::try_from(parsed.ehdr.e_phoff).unwrap() + stack * size_of::<libc::Elf64_Phdr>();
    let flags = phdr + offset_of!(libc::Elf64_Phdr, p_flags);
    let value = u32::from_le_bytes(bytes[flags..flags + 4].try_into().unwrap()) | elf::abi::PF_X;
    bytes[flags..flags + 4].copy_from_slice(&value.to_le_bytes());
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    let output = run_loaded_fixture(file.path().to_str().unwrap(), true, false);
    assert_eq!(output.status.code(), Some(37), "{output:?}");
    assert_eq!(output.stdout, b"runtime-ok\n");
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
#[timeout(10_000)]
fn does_not_require_section_headers() {
    let mut bytes = std::fs::read(FIXTURE_DYN_PIE).unwrap();
    // The ELF loader must not use section metadata, even if it is unusable.
    write_u64(&mut bytes, offset_of!(libc::Elf64_Ehdr, e_shoff), u64::MAX);
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    let output = run_loaded_fixture(file.path().to_str().unwrap(), true, false);
    assert_eq!(output.status.code(), Some(37), "{output:?}");
    assert_eq!(output.stdout, b"runtime-ok\n");
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
#[timeout(10_000)]
fn panic_terminates_with_read_only_stderr() {
    let mut command = launcher();
    // Keep the descriptor valid, but make writes fail with EBADF.
    command.stderr(std::fs::File::open("/dev/null").unwrap());
    // No target argument: the launcher panics while reading its arguments.
    let output = output(command);
    assert_eq!(output.status.code(), Some(101));
    assert!(output.stderr.is_empty());
}

fn launcher() -> Command {
    let mut command = Command::new(LAUNCHER);
    command.stderr(Stdio::piped());
    command
}

fn assert_image(path: &str, image_type: u16, interpreter: bool) {
    let bytes = std::fs::read(path).unwrap();
    let elf = elf::ElfBytes::<elf::endian::LittleEndian>::minimal_parse(&bytes).unwrap();
    assert_eq!(elf.ehdr.e_type, image_type, "unexpected fixture image type: {path}");
    let has_interp = elf.segments().unwrap().iter().any(|ph| ph.p_type == elf::abi::PT_INTERP);
    assert_eq!(has_interp, interpreter, "unexpected PT_INTERP: {path}");
}

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + size_of::<u16>()].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + size_of::<u64>()].copy_from_slice(&value.to_le_bytes());
}

fn output(mut command: Command) -> Output {
    command.stdin(Stdio::null()).output().expect("cannot run freestanding launcher")
}
