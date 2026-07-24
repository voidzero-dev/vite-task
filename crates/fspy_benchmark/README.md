# fspy benchmark

Measures fspy's wall-clock overhead while multiple threads repeatedly open a nonexistent path.
Criterion reports the nonnegative difference between tracked and untracked runs.

Linux measures dynamic and static targets. macOS and Windows measure their preload implementations.

Run the benchmark with:

```sh
just benchmark-fspy
```
