# fspy benchmark

Measures what fspy costs a process it tracks, and whether a change to fspy moved that cost. The
cost is two separable things, and each is measured apart from the other so that neither dilutes it:

- `launch` is the wall clock of a launch that opens nothing, so it prices starting a process under
  tracking: injection, session setup, and teardown.
- `access` is how long a batch of opens takes, timed by each of two threads opening a path of
  their own, so it prices interception under the concurrency a tracked process actually has,
  rather than the launch around it.

Linux measures a dynamically linked target that exercises `LD_PRELOAD` plus a
`x86_64-unknown-linux-musl` target that exercises seccomp user notification. macOS measures
`DYLD_INSERT_LIBRARIES` and Windows measures Detours injection.

Run the overhead report locally with:

```sh
just benchmark-fspy
```

## How regressions are caught

Comparing a pull request run with a baseline recorded by an earlier run on `main` cannot work,
and every attempt to make it work failed the same way: two runs land on two runner instances, and
identically configured instances disagree — on absolute times by tens of percent, and still by up
to ten percent after normalizing every measurement against an untracked launch on the same
machine. A threshold above that noise is a threshold above any regression worth catching.

So nothing is ever compared across runs. The pieces are split so that the two fspy revisions being
compared can run side by side on one machine:

- `fspy_benchmark_target` (and its static twin) is the workload: it opens missing paths from a
  configurable number of threads and reports the median batch time on stdout. Batches are a few
  microseconds long, so the median falls on batches the scheduler left alone; a whole-run wall
  clock would sum every disturbance instead. It does not link fspy.
- `fspy_benchmark_launcher` runs the target once — tracked through `fspy::Command`, or untracked
  for context — and prints the launch wall clock plus the target's numbers. It is the only piece
  that links fspy.
- `fspy_benchmark` is the harness. CI builds the launcher twice, against the fspy under review and
  against the merge-base fspy (from the same launcher source, so both revisions are measured by
  identical code), then the harness launches both builds interleaved, back to back, cycling
  every ordering. Whatever the runner instance is doing to the numbers, it is doing to both arms.

Each iteration yields the ratio of the head launch to the base launch per metric; the reported
change is the median of those ratios. An untracked arm runs in the same rotation and prices
tracking itself — that overhead number is context, not a gate. A row whose median change exceeds
its threshold fails the job.

Locally there is no second fspy build, so `just benchmark-fspy` reports only the overhead column.
Point `--base-launcher` at another build of `fspy_benchmark_launcher` to compare two revisions by
hand.

Because the launcher is compiled against both revisions from the head's source, a pull request
that changes the parts of the `fspy::Command` API the launcher uses can fail the baseline build.
Keep the launcher's API surface minimal; if the API must change incompatibly, the benchmark run
for that one pull request is expected to fail its baseline build.
