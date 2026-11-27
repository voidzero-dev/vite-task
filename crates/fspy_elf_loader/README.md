# fspy_elf_loader

Linux userland exec for fspy, the file tracker behind `vp run`: map a target
ELF into the current process and jump to it, so intercept state installed
before entry (SIGSYS, handlers, mappings) survives.

The **Linux end state** is this path alone. Kernel `execve` of the target
would reset intercept state, so fspy must not exec the target. Resolve PATH
and shebang in userland, kernel-exec a freestanding launcher, then
[`exec_elf`](src/lib.rs). That replaces today's `LD_PRELOAD` vs
`seccomp_unotify` split (chosen from `PT_INTERP`). SIGSYS stays as the
intercept; it is not a second spawn backend.

This is a **compatibility contract**, not a complete `execve`. This crate
only maps and enters an ELF. PATH, shebang, policy, nested exec, and fspy
wiring belong above it. fspy already parses shebang in `fspy_shared_unix`;
the kernel will not, once it never sees the target.

## Support

Native little-endian ELF64 on x86-64 and AArch64. An executable `PT_GNU_STACK`
maps the stack executable, as Linux does. `ET_EXEC` that overlaps any retained
mapping is rejected.

|           | `PT_INTERP`                                       | no `PT_INTERP`                                     |
| --------- | ------------------------------------------------- | -------------------------------------------------- |
| `ET_DYN`  | dynamic PIE: enter ld.so, `AT_BASE` = interp bias | static-PIE / ld.so: enter `e_entry`, `AT_BASE` = 0 |
| `ET_EXEC` | dynamic non-PIE: same as dynamic PIE              | traditional static: enter `e_entry`, `AT_BASE` = 0 |

All four cells load. With an interpreter, map both images and enter the
interpreter even though `AT_ENTRY` names the executable. Without one, map only
the executable.

The launcher is itself a static `ET_EXEC` at a high fixed address so the low
4 GiB stays free for non-PIE targets:

| Arch    | Base                        |
| ------- | --------------------------- |
| x86-64  | `0x5500_0000_0000` (85 TiB) |
| AArch64 | `0x0002_0000_0000` (8 GiB)  |

These sit in current LLVM Linux ASan/TSan application regions; GCC 12's AArch64
TSan still rejects the 8 GiB mapping. The addresses are conventions, not
reserved ranges.

## Done

- `PT_LOAD` mapping in the Linux `elf_map` shape (shared pages, BSS, padzero).
- Optional `PT_INTERP`, including padded interpreter paths.
- Startup stack: contiguous argv/envp strings, rebuilt auxv, copied
  `AT_RANDOM` / platform strings, inherited scalars and resident vDSO.
- Naked enter stubs: install SP, clear dirty GPRs, jump (no return address).
- `SIGSYS` signal, `/proc/*/exe`, and custom `SyscallInterceptor` virtualization
  with a private pooled `SIGSYS` stack, virtualized `rt_sigaction` /
  `rt_sigprocmask` / `rt_sigpending` / masked waits / `fork` / `vfork`, and
  direct user memory copies that assume valid pointers.
- Wired into `fspy` via `fspy_preload_linux` as the sole Linux tracking backend.
- [`fspy_elf_loader_tests`](../fspy_elf_loader_tests/README.md): dynamic and
  static PIE/`ET_EXEC` runtimes, overlap rejection, malformed images, and
  `SIGSYS` + `/proc/*/exe` virtualization coverage.

## Not a complete exec

Do not try to close these in this crate unless a real target requires it:

- **`brk` / `start_brk`.** The kernel's brk belongs to the high launcher
  mapping. It cannot be moved to a Linux-faithful image end. glibc malloc
  still works via mmap; leave brk alone.
- **External `/proc` views.** `/proc/self/exe` and `/proc/<pid>/exe` are
  virtualized inside the tracked process, while external `/proc/<pid>/exe`
  observers still see the launcher binary.
- **Privileged / unsupported binaries.** `PR_SET_NO_NEW_PRIVS` is enabled for
  seccomp (so setuid/setgid and file-capability escalations do not apply), and
  ELF32 and `binfmt_misc` binaries are rejected with `ENOEXEC`.
- **Execute-only (`0111`) binaries.** Userland ELF mapping opens the executable
  with `O_RDONLY` to validate headers and `mmap` segments, so execute-only
  (`chmod 0111`) binaries cannot be mapped and fail with `EACCES`.
- **Launcher path visibility across mount changes.** Every tracked `execve`
  re-enters the materialized freestanding launcher binary on disk. If a tracked
  process tree changes its mount namespace or root (`chroot`, `pivot_root`, or
  a private `/tmp` mount hiding the materialized launcher path), subsequent
  child `execve` calls fail with `ENOENT`.
- GNU properties, other arches, growable stack (fixed 8–64 MiB from
  `RLIMIT_STACK` + guard), and personality cleanup.

Load failures panic and terminate. `exec_elf` is for a fresh single-threaded
process with no libc or TLS init. Trusted images only; this is not a sandbox.
