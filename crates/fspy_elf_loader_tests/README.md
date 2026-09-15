# fspy_elf_loader_tests

Integration tests for [`fspy_elf_loader`](../fspy_elf_loader/README.md).

The tests check whether programs start and behave as expected through the custom loader. They help catch compatibility regressions as its scope grows.

## Scope

This crate covers program execution and loading failures in Linux processes. Unit tests for individual loader components stay in `fspy_elf_loader`.

Keeping integration tests in a separate crate lets us use the normal Rust test runtime without adding it to the loader. The tests validate the loader itself; full fspy interception remains outside their scope.
