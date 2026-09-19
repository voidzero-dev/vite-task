# fspy_elf_loader

An experimental Linux program loader for fspy's custom-loader approach.

fspy needs to set up interception before a target program starts and keep that setup intact as the program takes over. This crate explores loading the program within the existing process, preserving the signal handlers that a normal exec would reset.

## Scope

This crate handles loading an ELF program and transferring execution to it. The caller owns execution-policy checks and interception setup. Integration with fspy remains separate work.

The current implementation is a proof of concept for evaluating compatibility. It does not yet provide a complete replacement for Linux exec.

It loads native ELF64 dynamic executables on AArch64 and x86-64, including PIE (`ET_DYN`) and fixed-address (`ET_EXEC`) images. Fixed-address images are rejected if their address range overlaps any existing mapping. Static executables remain unsupported.

[`fspy_elf_loader_tests`](../fspy_elf_loader_tests/README.md) covers program execution through the loader.
