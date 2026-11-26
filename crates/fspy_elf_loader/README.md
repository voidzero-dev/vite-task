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

Native little-endian ELF64 on x86-64 and AArch64. `PT_GNU_STACK` must be
non-executable. `ET_EXEC` that overlaps any retained mapping is rejected.

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
- [`fspy_elf_loader_tests`](../fspy_elf_loader_tests/README.md): dynamic and
  static PIE/`ET_EXEC` runtimes, overlap rejection, malformed images. An
  ignored Node test exists (`--include-ignored` on Linux with Node on `PATH`).

## Not a complete exec

Do not try to close these in this crate unless a real target requires it:

- **`brk` / `start_brk`.** The kernel's brk belongs to the high launcher
  mapping. It cannot be moved to a Linux-faithful image end. glibc malloc
  still works via mmap; leave brk alone.
- **`/proc` identity.** `/proc/self/exe` and parent-visible `/proc/<pid>/exe`
  still name the launcher. Parent `/proc/<pid>/exe` is not a JS/Chromium
  identity gate. `/proc/self/exe` is, and needs virtualization later.
- GNU properties, executable stacks, ELF32, other arches, growable stack
  (fixed 8 MiB + guard), fd/signal/personality cleanup.

Load failures panic and terminate. `exec_elf` is for a fresh single-threaded
process with no libc or TLS init. Trusted images only; this is not a sandbox.

## Product work (elsewhere)

fspy still spawns with `LD_PRELOAD` or `seccomp_unotify`. The loader is not
wired in. Remaining work is to make launcher + userland shebang + `exec_elf`
the only Linux spawn:

1. Shebang and PATH above `exec_elf`, then load the resulting ELF. Scripts
   never hit kernel `binfmt_script`. Nested `execve` must re-enter this path
   instead of kernel-execing the child image.
2. Virtualize `/proc/self/exe` so the target sees its own path (`process.execPath`
   / `fs.realpath`). The ignored Node test does not do this.
3. Drop the Linux preload/seccomp spawn split. One intercept setup in the
   launcher covers dynamic glibc, musl, and static images.
