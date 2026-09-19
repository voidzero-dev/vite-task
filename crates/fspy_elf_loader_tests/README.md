# fspy_elf_loader_tests

Integration tests for [`fspy_elf_loader`](../fspy_elf_loader/README.md).

The tests check whether programs start and behave as expected through the custom loader. They help catch compatibility regressions as its scope grows.

The default suite covers dynamic PIE and fixed-address executables, including address collisions. With Node.js on `PATH`, run `cargo test -p fspy_elf_loader_tests -- --include-ignored` on Linux to also compare a real Node program with normal execution.

## Scope

This crate covers program execution and loading failures in Linux processes. Unit tests for individual loader components stay in `fspy_elf_loader`.

Keeping integration tests in a separate crate lets us use the normal Rust test runtime without adding it to the loader. The tests validate the loader itself; full fspy interception remains outside their scope.
