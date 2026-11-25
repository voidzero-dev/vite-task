# fspy_elf_loader_tests

Integration tests for [`fspy_elf_loader`](../fspy_elf_loader/README.md), executed via the freestanding [`fspy_elf_loader_test_launcher`](../fspy_elf_loader_test_launcher/src/main.rs) binary.

The tests check whether programs start and behave as expected through the custom loader. They help catch compatibility regressions as its scope grows.

The default suite compiles a unified C runtime fixture (`fixtures/runtime.c`) across all four ELF64 cells — dynamic PIE, dynamic fixed-address (`ET_EXEC`), static fixed-address, and static-PIE — and compares each fixture differentially against direct kernel `execve`. It also covers rejection of a target overlapping the launcher, malformed ELF images, and stripped section headers. With Node.js on `PATH`, run `cargo test -p fspy_elf_loader_tests -- --include-ignored` on Linux to also compare a real Node program with normal execution.

The launcher artifact is checked on every host for a static fixed-address
layout at 85 TiB on x86-64 or 8 GiB on AArch64, with no dynamic relocation
requirements. The collision test checks that the launcher still reports the
expected error after rejecting an overlapping target. CI runs the Linux suite
in both debug and release profiles (glibc and musl).

## Scope

This crate covers program execution and loading failures in Linux processes. Unit tests for individual loader components stay in `fspy_elf_loader`.

Keeping the freestanding test launcher and integration tests in separate crates keeps `fspy_elf_loader` a pure `#![no_std]` library while letting integration tests use the normal Rust test runtime. The tests validate the loader itself; full fspy interception remains outside their scope.
