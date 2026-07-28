# fspy benchmark

Measures what fspy costs a process it tracks, and whether a change to fspy moved that cost. It reports two rows:

- `launch`: the wall clock of a tracked launch that opens nothing. This is the cost of starting a process under tracking: injection, session setup, and teardown.
- `access`: how long a batch of opens takes, timed by two threads that each open their own path. This is the cost of interception itself, under the concurrency a tracked process normally has.

Linux measures a dynamically linked target (`LD_PRELOAD`) and a `x86_64-unknown-linux-musl` target (seccomp user notification). macOS measures `DYLD_INSERT_LIBRARIES`. Windows measures Detours injection.

Run the overhead report locally with:

```sh
just benchmark-fspy
```

## How regressions are caught

Comparing a pull request run against a saved result from an earlier `main` run does not work. The two runs land on different runner instances, and identical instances disagree by more than any regression worth catching: tens of percent on absolute times, and still up to ten percent after normalizing against an untracked launch. A threshold above that noise is a threshold above every real regression.

So nothing is ever compared across runs. Both fspy revisions run side by side on one machine, in one job. Three pieces make that possible:

- `fspy_benchmark_target` (and its static twin) is the workload. It opens missing paths from several threads and prints the median batch time. It does not link fspy. Batches are a few microseconds long, so the median lands on batches the scheduler left alone; a whole-run wall clock would add up every disturbance instead.
- `fspy_benchmark_launcher` runs the target once, tracked through `fspy::Command` or untracked, and prints the launch wall clock plus the target's number. It is the only piece that links fspy.
- `fspy_benchmark` is the harness. CI builds the launcher twice: against the fspy under review, and against the merge-base fspy. Both builds use the same launcher source, so both revisions are measured by identical code. The harness then launches both builds back to back, cycling every ordering. Whatever the runner does to the numbers, it does to both.

Each iteration gives one head/base ratio per row. The reported change is the median of those ratios, printed with its quartile spread. The benchmark only reports; it never fails the job. Running the same fspy on both sides stays within a couple of percent, so read a change well past that as real.

An untracked launch runs in the same rotation. It prices tracking itself, and is reported as context, never gated.

Locally there is no second fspy build, so `just benchmark-fspy` reports only the overhead column. Pass `--base-launcher` with another build of `fspy_benchmark_launcher` to compare two revisions by hand.

One caveat: the launcher is compiled against both revisions from the head's source. A pull request that breaks the small part of the `fspy::Command` API the launcher uses will fail the baseline build. Keep that API surface small; if it must break, expect a red benchmark on that one pull request.
