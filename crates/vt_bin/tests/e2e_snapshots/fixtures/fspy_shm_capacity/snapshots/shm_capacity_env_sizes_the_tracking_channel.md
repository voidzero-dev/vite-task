# shm_capacity_env_sizes_the_tracking_channel

`VP_RUN_INTERNAL_FSPY_SHM_CAPACITY` sizes the shared memory a tracked task reports its file accesses through. The task makes twenty thousand of them, and this run leaves 64 KiB for them once the descriptor table has taken its half-gibibyte of sparse address space, which is nowhere near enough.

The task runs to the end anyway: recording must never stop the program doing the work, so the accesses past that go unrecorded and the process carries on to a clean exit, printing its last line. What the run cannot claim is that it saw every file the task touched, so it is not cached, and the second run says the same rather than replaying an entry built from part of a trace.

Not on musl, which has no preload: those builds collect through the seccomp supervisor, on the runner's own side of the boundary, so they have no shared-memory channel to fill.

## `VP_RUN_INTERNAL_FSPY_SHM_CAPACITY=536936448 vt run -v stat`

512 MiB of table and 64 KiB of room for records

```
$ vtt stat-many 20000
stat 20000


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] fspy-shm-capacity#stat: $ vtt stat-many 20000 ✓
      → Not cached: this task used more files than automatic tracking can record. Configure `input` and `output` manually to enable caching.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `VP_RUN_INTERNAL_FSPY_SHM_CAPACITY=536936448 vt run -v stat`

nothing was cached to replay

```
$ vtt stat-many 20000
stat 20000


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] fspy-shm-capacity#stat: $ vtt stat-many 20000 ✓
      → Not cached: this task used more files than automatic tracking can record. Configure `input` and `output` manually to enable caching.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
