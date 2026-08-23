# Copilot coding-agent ptrace strict results

Direct GitHub Copilot coding-agent execution in the checked-out `voidzero-dev/vite-task` repository, using the ordinary agent user and without changing product code, workflows, seccomp, Yama, capabilities, or installing software.

## Commit and source selection

```text
$ git rev-parse HEAD
73391a2afe621f1bdf0f81857751abda85d65007
[exit:0]

$ grep -nE 'required-summary|ptrace-word-io' research/ptrace-environment-probe.c
36:    report("required-summary", !required_failed, 0,
179:        report_required("ptrace-word-io", word_ok, word_error,
183:        report_required("ptrace-word-io", false, error,
[exit:0]

$ grep -nE 'ptrace::(cont|step)|patch_syscall_and_trap' crates/inject_demo/src/main.rs
163:        write_word(pid, pc, arch::patch_syscall_and_trap(original))?;
168:        ptrace::cont(pid, None)?;
257:        pub const fn patch_syscall_and_trap(word: u64) -> u64 {
300:        pub const fn patch_syscall_and_trap(_word: u64) -> u64 {
[exit:0]
```

Confirmed from source: `remote_mmap` uses `ptrace::cont(pid, None)?;` plus `patch_syscall_and_trap(...)`, not the old `PTRACE_SINGLESTEP` path.

## Environment capture

```text
$ uname -a
Linux runnervm76f27 6.17.0-1022-azure #22-Ubuntu SMP Mon Jul 27 17:24:03 UTC 2026 x86_64 x86_64 x86_64 GNU/Linux
[exit:0]

$ cat /etc/os-release
PRETTY_NAME="Ubuntu 24.04.4 LTS"
NAME="Ubuntu"
VERSION_ID="24.04"
VERSION="24.04.4 LTS (Noble Numbat)"
VERSION_CODENAME=noble
ID=ubuntu
ID_LIKE=debian
HOME_URL="https://www.ubuntu.com/"
SUPPORT_URL="https://help.ubuntu.com/"
BUG_REPORT_URL="https://bugs.launchpad.net/ubuntu/"
PRIVACY_POLICY_URL="https://www.ubuntu.com/legal/terms-and-policies/privacy-policy"
UBUNTU_CODENAME=noble
LOGO=ubuntu-logo
[exit:0]

$ id
uid=1001(runner) gid=1001(runner) groups=1001(runner),4(adm),100(users),118(docker),999(systemd-journal)
[exit:0]

$ grep -E "^(CapEff|NoNewPrivs|Seccomp|Seccomp_filters):" /proc/self/status
CapEff:	0000000000000000
NoNewPrivs:	0
Seccomp:	0
Seccomp_filters:	0
[exit:0]

$ cat /proc/sys/kernel/yama/ptrace_scope
1
[exit:0]

$ cat /proc/self/cgroup
0::/user.slice/user-0.slice/session-c1.scope/ebpf-cgroup-firewall
[exit:0]

$ cat /proc/1/cgroup
0::/init.scope
[exit:0]

$ test -e /.dockerenv
[exit:1]

$ test -e /run/.containerenv
[exit:1]

$ command -v systemd-detect-virt
/usr/bin/systemd-detect-virt
[exit:0]

$ systemd-detect-virt -c
none
[exit:1]
```

## Exact probe compile

```text
$ gcc -O2 -Wall -Wextra -Werror research/ptrace-environment-probe.c -o /tmp/ptrace-environment-probe
[exit:0]
```

## Exact probe run

```text
$ /tmp/ptrace-environment-probe
pid=3960 uid=1001 euid=1001
CapEff:	0000000000000000
NoNewPrivs:	0
Seccomp:	0
YamaScope:	1
traceme+exec+regset            result=PASS errno=0 (none) SIGTRAP exec-stop
attach-direct-child            result=PASS errno=0 (none) 
seize-direct-child             result=PASS errno=0 (none) 
process-vm-write               result=PASS errno=0 (none) direct child
process-vm-read                result=PASS errno=0 (none) direct child
ptrace-word-io                 result=PASS errno=0 (none) stopped direct child
seize-live-grandchild          result=PASS errno=0 (none) ancestor, not direct parent
seize-sibling                  result=FAIL errno=1 (Operation not permitted) same UID
seize-sibling-pr-set-ptracer   result=PASS errno=0 (none) target opted in
seize-dumpable-zero-child      result=FAIL errno=1 (Operation not permitted) 
seize-orphan-no-subreaper      result=FAIL errno=1 (Operation not permitted) reparented away
seize-orphan-subreaper         result=PASS errno=0 (none) reparented to supervisor
required-summary               result=PASS errno=0 (none) 
[exit:0]
```

Required probe verdict: **successful**, because both `required-summary result=PASS` and `[exit:0]` are present.

Interpretation from the probe output:
- Required positives: `traceme+exec+regset`, `seize-direct-child`, `ptrace-word-io`, and the final `required-summary` all passed.
- `process-vm-write` and `process-vm-read` also passed, but these are reported separately from the required `PTRACE_PEEKDATA`/`PTRACE_POKEDATA` fallback path (`ptrace-word-io`).
- Deliberately negative relationship/dumpability/Yama checks: `seize-sibling`, `seize-dumpable-zero-child`, and `seize-orphan-no-subreaper` failed with `EPERM`, while the opt-in/ancestor/subreaper variants passed.

## Exact inject_demo build/run

```text
$ cargo run --locked -p inject_demo
info: syncing channel updates for nightly-2026-08-02-x86_64-unknown-linux-gnu
info: latest update on 2026-08-02 for version 1.99.0-nightly (73dc9167f 2026-08-01)
info: downloading 6 components
warning: Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: workspace (manifest) generated 1 warning
warning: crates/fspy_e2e/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_e2e` (manifest) generated 1 warning
warning: crates/fspy_test_bin/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_test_bin` (manifest) generated 1 warning
warning: crates/socket_ipc/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `socket_ipc` (manifest) generated 1 warning
warning: crates/vt_graph/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_graph` (manifest) generated 1 warning
warning: crates/vt_graph_ser/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_graph_ser` (manifest) generated 1 warning
warning: crates/pty_terminal_test/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `pty_terminal_test` (manifest) generated 1 warning
warning: crates/vt_shell/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_shell` (manifest) generated 1 warning
warning: crates/vt_server/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_server` (manifest) generated 1 warning
warning: crates/vt_tui/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_tui` (manifest) generated 1 warning
warning: crates/fspy_nostd_alloc/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_nostd_alloc` (manifest) generated 1 warning
warning: crates/vt_workspace/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_workspace` (manifest) generated 1 warning
warning: crates/fspy_nostd/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_nostd` (manifest) generated 1 warning
warning: crates/snapshot_test/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `snapshot_test` (manifest) generated 1 warning
warning: crates/fspy_ipc_str/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_ipc_str` (manifest) generated 1 warning
warning: crates/preload_test_lib/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `preload_test_lib` (manifest) generated 1 warning
warning: crates/fspy_benchmark/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_benchmark` (manifest) generated 1 warning
warning: crates/vt_glob/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_glob` (manifest) generated 1 warning
warning: crates/fspy_detours_sys/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_detours_sys` (manifest) generated 1 warning
warning: crates/fspy_preload_unix/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_preload_unix` (manifest) generated 1 warning
warning: crates/materialized_artifact_macros/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `materialized_artifact_macros` (manifest) generated 1 warning
warning: crates/vt/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt` (manifest) generated 1 warning
warning: crates/vt_select/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_select` (manifest) generated 1 warning
warning: crates/fspy_shared/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_shared` (manifest) generated 1 warning
warning: crates/vt_ipc_shared/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_ipc_shared` (manifest) generated 1 warning
warning: crates/fspy_preload_windows/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_preload_windows` (manifest) generated 1 warning
warning: crates/fspy_benchmark_static_target/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_benchmark_static_target` (manifest) generated 1 warning
warning: crates/vt_client_napi/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_client_napi` (manifest) generated 1 warning
warning: crates/vt_plan/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_plan` (manifest) generated 1 warning
warning: crates/fspy_shm/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_shm` (manifest) generated 1 warning
warning: crates/fspy_blob/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_blob` (manifest) generated 1 warning
warning: crates/pty_terminal_test_client/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `pty_terminal_test_client` (manifest) generated 1 warning
warning: crates/vt_powershell/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_powershell` (manifest) generated 1 warning
warning: crates/vt_bin/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_bin` (manifest) generated 1 warning
warning: crates/inject_demo/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `inject_demo` (manifest) generated 1 warning
warning: crates/fspy_benchmark_launcher/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_benchmark_launcher` (manifest) generated 1 warning
warning: crates/fspy_shared_unix/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_shared_unix` (manifest) generated 1 warning
warning: crates/subprocess_test/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `subprocess_test` (manifest) generated 1 warning
warning: crates/fspy_preload_linux/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_preload_linux` (manifest) generated 1 warning
warning: crates/vt_client/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_client` (manifest) generated 1 warning
warning: crates/materialized_artifact/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `materialized_artifact` (manifest) generated 1 warning
warning: crates/fspy_benchmark_target/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_benchmark_target` (manifest) generated 1 warning
warning: crates/fspy/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy` (manifest) generated 1 warning
warning: crates/fspy_seccomp_unotify/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_seccomp_unotify` (manifest) generated 1 warning
warning: crates/vt_str/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_str` (manifest) generated 1 warning
warning: crates/fspy_client_unix/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `fspy_client_unix` (manifest) generated 1 warning
warning: crates/pty_terminal/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `pty_terminal` (manifest) generated 1 warning
warning: crates/vt_path/Cargo.toml: `lints.rust.unit-bindings` is deprecated in favor of `lints.rust.unit_bindings` and will not work in a future edition
warning: `vt_path` (manifest) generated 1 warning
    Updating crates.io index
    Updating git repository `https://github.com/polachok/passfd`
    Updating git repository `https://github.com/rust-vmm/seccompiler`
    Updating git submodule `https://github.com/rust-vmm/rust-vmm-ci.git`
 Downloading crates ...
  Downloaded cfg-if v1.0.4
  Downloaded getrandom v0.4.2
  Downloaded memchr v2.8.0
  Downloaded bitflags v2.10.0
  Downloaded anyhow v1.0.103
  Downloaded cfg_aliases v0.2.1
  Downloaded once_cell v1.21.3
  Downloaded autocfg v1.5.0
  Downloaded fastrand v2.3.0
  Downloaded proc-macro2 v1.0.106
  Downloaded quote v1.0.45
  Downloaded atoi v3.1.0
  Downloaded errno v0.3.14
  Downloaded plain v0.2.3
  Downloaded scroll v0.13.0
  Downloaded scroll_derive v0.13.1
  Downloaded tempfile v3.25.0
  Downloaded num-traits v0.2.19
  Downloaded log v0.4.29
  Downloaded syscalls v0.8.1
  Downloaded unicode-ident v1.0.23
  Downloaded syn v2.0.117
  Downloaded bstr v1.12.1
  Downloaded goblin v0.10.7
  Downloaded rustix v1.1.3
  Downloaded nix v0.31.2
  Downloaded libc v0.2.185
  Downloaded linux-raw-sys v0.12.1
  Downloaded linux-raw-sys v0.11.0
   Compiling proc-macro2 v1.0.106
   Compiling unicode-ident v1.0.23
   Compiling autocfg v1.5.0
   Compiling quote v1.0.45
   Compiling libc v0.2.185
   Compiling num-traits v0.2.19
   Compiling syscalls v0.8.1
   Compiling fspy_preload_linux v0.0.0 (/home/runner/work/vite-task/vite-task/crates/fspy_preload_linux)
   Compiling memchr v2.8.0
error[E0463]: can't find crate for `core`
  |
  = note: the `x86_64-unknown-none` target may not be installed
  = help: consider downloading the target with `rustup target add x86_64-unknown-none`
  = help: consider building the standard library from source with `cargo build -Zbuild-std`

For more information about this error, try `rustc --explain E0463`.
error: could not compile `memchr` (lib) due to 1 previous error
warning: build failed, waiting for other jobs to finish...
error: could not compile `num-traits` (lib) due to 1 previous error
[exit:101]
```

`cargo run --locked -p inject_demo` did not complete in this environment. Exact blocker preserved above: `error[E0463]: can't find crate for core` with note `the x86_64-unknown-none target may not be installed`.
