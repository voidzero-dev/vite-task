# fspy_elf_loader

An experimental Linux program loader for fspy's custom-loader approach.

fspy needs to set up interception before a target program starts and keep that setup intact as the program takes over. This crate explores loading the program within the existing process, preserving the signal handlers that a normal exec would reset.

## Scope

This crate handles loading an ELF program and transferring execution to it. The caller owns execution-policy checks and interception setup. Integration with fspy remains separate work.

The current implementation is a proof of concept for evaluating compatibility. It does not yet provide a complete replacement for Linux exec.

It loads native ELF64 dynamic executables on AArch64 and x86-64, including PIE (`ET_DYN`) and fixed-address (`ET_EXEC`) images. Fixed-address images are rejected if their address range overlaps any existing mapping. Static executables remain unsupported.

The freestanding launcher is a static fixed-address executable with no startup relocation:

| Architecture | Base address       | Base in binary units |
| ------------ | ------------------ | -------------------- |
| x86-64       | `0x5500_0000_0000` | 85 TiB               |
| AArch64      | `0x0002_0000_0000` | 8 GiB                |

These bases leave the low 4 GiB available and sit in application regions of
the current LLVM Linux [AddressSanitizer](https://github.com/llvm/llvm-project/blob/main/compiler-rt/lib/asan/asan_mapping.h)
and [C/C++ ThreadSanitizer](https://github.com/llvm/llvm-project/blob/main/compiler-rt/lib/tsan/rtl/tsan_platform.h)
layouts. AArch64 uses a lower base to fit smaller virtual address spaces,
including 39-bit configurations, and remains below their 20 GiB TSan low
application-region limit. These placement choices do not establish complete
sanitizer compatibility: GCC 12's AArch64 TSan runtime rejects the 8 GiB
launcher mapping. Its [older layout](https://github.com/gcc-mirror/gcc/blob/releases/gcc-12.2.0/libsanitizer/tsan/tsan_platform.h)
limits low application memory to below 4 GiB on 39-bit systems and below
8 GiB on 48-bit systems. The addresses are conventions, not reserved ranges:
fixed-address targets that overlap retained mappings are still rejected.

[`fspy_elf_loader_tests`](../fspy_elf_loader_tests/README.md) covers program execution through the loader.
