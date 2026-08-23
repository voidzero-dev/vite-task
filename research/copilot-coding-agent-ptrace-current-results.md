# Copilot Coding Agent — ptrace Compatibility Experiment

**Date:** 2026-08-23  
**Environment:** GitHub Copilot Coding Agent (direct execution, not GitHub Actions)  
**Operator identity:** `uid=1001(runner) gid=1001(runner) groups=1001(runner),4(adm),100(users),118(docker),999(systemd-journal)`

---

## Commit SHA

```
git rev-parse HEAD
c41a2ae5d1b09f2c9071c1bf3fc847a670892639
```

## Source Verification

```
grep -nE 'required-summary|ptrace-word-io' research/ptrace-environment-probe.c
36:    report("required-summary", !required_failed, 0,
179:        report_required("ptrace-word-io", word_ok, word_error,
183:        report_required("ptrace-word-io", false, error,

grep -nE 'ptrace::(cont|step)|patch_syscall_and_trap' crates/inject_demo/src/main.rs
163:        write_word(pid, pc, arch::patch_syscall_and_trap(original))?;
168:        ptrace::cont(pid, None)?;
257:        pub const fn patch_syscall_and_trap(word: u64) -> u64 {
300:        pub const fn patch_syscall_and_trap(_word: u64) -> u64 {
```

**remote_mmap check:** `inject_demo` uses `ptrace::cont` at line 168. No `ptrace::step` present in `main.rs`. Proceeding (not aborted).

---

## Environment

### uname -a
```
Linux runnervm76f27 6.17.0-1022-azure #22-Ubuntu SMP Mon Jul 27 17:24:03 UTC 2026 x86_64 x86_64 x86_64 GNU/Linux
```

### /etc/os-release
```
PRETTY_NAME="Ubuntu 24.04.4 LTS"
NAME="Ubuntu"
VERSION_ID="24.04"
VERSION="24.04.4 LTS (Noble Numbat)"
VERSION_CODENAME=noble
ID=ubuntu
ID_LIKE=debian
```

### id
```
uid=1001(runner) gid=1001(runner) groups=1001(runner),4(adm),100(users),118(docker),999(systemd-journal)
```

### /proc/self/status security fields
```
CapEff:         0000000000000000
NoNewPrivs:     0
Seccomp:        0
Seccomp_filters: 0
```

### Yama ptrace_scope
```
cat /proc/sys/kernel/yama/ptrace_scope  →  1   (exit 0)
```

Yama scope 1 = "restricted ptrace" (only direct-parent relationships or PR_SET_PTRACER opt-in allowed without privilege). The probe handles this correctly.

### cgroup files

```
cat /proc/self/cgroup
0::/user.slice/user-0.slice/session-c1.scope/ebpf-cgroup-firewall

cat /sys/fs/cgroup/cgroup.controllers
cpuset cpu io memory hugetlb pids rdma misc dmem
```

### Container marker

```
ls /.dockerenv  →  absent (exit 1)
```

### systemd-detect-virt
```
microsoft   (exit 0)
```

Running inside a Microsoft Azure VM (not a container).

---

## ptrace-environment-probe.c

### Compile
```
gcc -O2 -Wall -Wextra -Werror research/ptrace-environment-probe.c -o /tmp/ptrace-environment-probe
exit: 0   (no output)
```

### Run
```
/tmp/ptrace-environment-probe
pid=3983 uid=1001 euid=1001
CapEff:         0000000000000000
NoNewPrivs:     0
Seccomp:        0
YamaScope:      1

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

exit: 0
```

**required-summary: PASS, exit 0 — required success criteria met.**

- `process-vm-*` (process_vm_read/write): PASS — separate from ptrace-word-io.
- `ptrace-word-io`: PASS — PTRACE_PEEKDATA/POKEDATA on stopped direct child.
- Expected relationship failures (Yama scope 1, no opt-in):
  - `seize-sibling`: FAIL — expected; sibling not a direct-parent relationship and no PR_SET_PTRACER.
  - `seize-dumpable-zero-child`: FAIL — expected; dumpable=0 blocks ptrace.
  - `seize-orphan-no-subreaper`: FAIL — expected; reparented away from tracer hierarchy.

---

## inject_demo (Rust injector)

### rustup target list --installed (before add)
```
x86_64-unknown-linux-gnu
```

`x86_64-unknown-none` was **not** installed.

### Allowed one-component setup
```
rustup target add x86_64-unknown-none
info: downloading component rust-std
exit: 0
```

### cargo run --locked -p inject_demo
```
cargo run --locked -p inject_demo
```

**Complete stdout/stderr (abridged — Cargo manifest warnings omitted for brevity):**

```
[... Cargo.toml lints.rust.unit-bindings deprecation warnings (harmless) ...]
    Updating crates.io index
    [downloaded and compiled: nix, goblin, seccompiler, inject_demo, fspy_preload_linux, ...]
    Finished `dev` profile [unoptimized] target(s) in 9.70s
     Running `target/debug/inject_demo`
payload: 40960 bytes, entry +0x3550, 141 relocations
mapped 41152 bytes into the target at 0x7f7cc7bda000
detached — payload will restore the exec context in-process
fspy_preload_linux: installed SIGSYS handler
openat: /home/runner/work/vite-task/vite-task/target/debug/glibc-hwcaps/x86-64-v3/libc.so.6
openat: /home/runner/work/vite-task/vite-task/target/debug/glibc-hwcaps/x86-64-v2/libc.so.6
openat: /home/runner/work/vite-task/vite-task/target/debug/libc.so.6
[... ld.so search path probes ...]
openat: /etc/ld.so.cache
openat: /lib/x86_64-linux-gnu/libc.so.6
openat: /usr/lib/locale/locale-archive
[... locale probes ...]
openat: test_path
SIGSYS works
/bin/cat exited with code 0
```

**exit: 0**

### Confirmation: PTRACE_CONT + syscall+trap, not SINGLESTEP

From source (`crates/inject_demo/src/main.rs` line 168):
```rust
ptrace::cont(pid, None)?;
```
and line 163:
```rust
write_word(pid, pc, arch::patch_syscall_and_trap(original))?;
```

The injector patches a syscall instruction and a trap (INT3/BRK) into the target's instruction stream, then resumes the target with **PTRACE_CONT** — not PTRACE_SINGLESTEP. Both steps (write_word patch + cont) completed successfully; the payload was mapped and executed, SIGSYS interception confirmed working, and the child `/bin/cat` exited with code 0.

---

## Summary

| Check | Result |
|-------|--------|
| Probe compile | ✅ exit 0 |
| required-summary | ✅ PASS |
| Probe exit | ✅ 0 |
| ptrace-word-io | ✅ PASS |
| process-vm-* | ✅ PASS (separate) |
| Expected relationship failures | ✅ seize-sibling, seize-dumpable-zero-child, seize-orphan-no-subreaper (Yama scope 1, no opt-in) |
| x86_64-unknown-none target pre-existing | ❌ absent |
| rustup target add x86_64-unknown-none | ✅ exit 0 (one allowed setup action) |
| inject_demo compile | ✅ exit 0 |
| inject_demo run | ✅ exit 0 |
| inject_demo uses PTRACE_CONT (not SINGLESTEP) | ✅ confirmed |
| inject_demo uses patch_syscall_and_trap | ✅ confirmed |

**ptrace is fully functional in this Copilot Coding Agent environment (Azure VM, Ubuntu 24.04, kernel 6.17, Yama scope 1, no seccomp, no containers).**
