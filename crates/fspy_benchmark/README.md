# fspy benchmark

Measures what fspy costs a process it tracks. That cost is two separable things, and each is
measured apart from the other so that neither dilutes it:

- `launch` is the wall clock of a launch that opens nothing, so it prices starting a process under
  tracking: injection, session setup, and teardown.
- `access` is how long a batch of opens takes, timed by the thread that runs them, so it prices
  interception itself rather than the launch around it.
- `access-tail` is the slow end of the same samples, which moves when accesses start to scatter
  rather than to cost more.
- `access-concurrent` is `access` with a second thread intercepting at the same time, each thread
  opening a path of its own.

Every sample runs tracked and untracked launches as interleaved pairs, alternating which one goes
first, and reports the median per-pair overhead: `tracked / untracked - 1`.

Linux measures a dynamically linked target that exercises `LD_PRELOAD` plus a
`x86_64-unknown-linux-musl` target that exercises seccomp user notification. macOS measures
`DYLD_INSERT_LIBRARIES` and Windows measures Detours injection.

Run the benchmark with:

```sh
just benchmark-fspy
```

## Comparing runs

CI stores the results of the latest `main` run and compares pull request runs against them, so the
numbers have to be reproducible on whichever runner instance a pull request happens to get.

Absolute times are not reproducible: identically configured runners disagree by tens of percent,
which is far more than the overhead being measured. Two things bring that back under control.

Overhead is measured against an untracked launch that ran back to back, and reported as a share of
it rather than as a difference, so that a given slowdown in fspy moves the number by the same amount
everywhere, however cheap tracking is compared with the work the target does.

Accesses are timed from inside the target rather than around the launch. A wall-clock launch is one
number covering milliseconds, so anything the runner does during it lands in the result; timing each
batch of eight opens instead produces hundreds of samples per launch, each a few microseconds long,
and the median discards the ones the scheduler spoiled. That took the spread from 4% to 0.7% on
macOS, and it made the benchmark faster, because a handful of launches now carries more information
than hundreds did.

What remains is a spread of about 1% to 4%, measured by benchmarking the same commit on six runner
instances per platform. The noise threshold is set above it, and Criterion still prints the change it
measured when it stays below the threshold. `launch` is the widest of them, at times 7% on Linux: it
is the one row that has to be a whole-process wall-clock measurement, so it is also the one most
exposed to the runner.

Two rows are left out where they do not reproduce:

- `access-concurrent` on macOS. A second busy thread there makes every open cost about three times
  as much, by an amount that changes from launch to launch, which moves the result by 25% between
  runner instances against 3% for one thread. The same measurement reproduces within 4% on Linux and
  Windows.
- `access-tail` on the seccomp target, whose slow samples are dominated by how quickly the runner
  wakes the supervisor and move by 10% between instances.
