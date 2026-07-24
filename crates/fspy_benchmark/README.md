# fspy benchmark

This benchmark measures the wall-clock overhead fspy adds to a filesystem-heavy child process.
The target starts four threads. Each thread opens 2,048 distinct, pre-created files for reading
and drops every handle immediately.

The fixture is created before Criterion starts measuring. Tracked and untracked runs execute the
same target with the same arguments, empty environment, working directory, and standard streams.
The timer covers spawning the target through process termination and fspy finalization. A tracked
warmup verifies that all fixture accesses were captured outside the timed section.

On Linux, the suite measures two targets:

- `dynamic` is dynamically linked and exercises `LD_PRELOAD`.
- `static` is linked for `x86_64-unknown-linux-musl` and exercises seccomp user notification.

macOS measures `DYLD_INSERT_LIBRARIES`, and Windows measures Detours injection.

Run the benchmark with:

```sh
just benchmark-fspy
```

CI runs it on the Linux, macOS, and Windows Namespace runner profiles. Each result is compared with
the latest `main` result.
