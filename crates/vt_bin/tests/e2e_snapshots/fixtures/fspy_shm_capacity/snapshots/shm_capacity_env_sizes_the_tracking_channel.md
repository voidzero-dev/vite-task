# shm_capacity_env_sizes_the_tracking_channel

`VP_RUN_INTERNAL_FSPY_SHM_CAPACITY` sizes the shared memory a tracked task reports its file accesses through. The task makes twenty thousand of them, and a 64 KiB channel holds a thousand: one descriptor slot per 64 bytes of the region.

The task runs to the end anyway: recording must never stop the program doing the work, so the accesses past that go unrecorded and the process carries on to a clean exit, printing its last line. What the run cannot claim is that it saw every file the task touched, so it is not cached, and the second run says the same rather than replaying an entry built from part of a trace.

Not on musl, which has no preload: those builds collect through the seccomp supervisor, on the runner's own side of the boundary, so they have no shared-memory channel to fill.

## `VP_RUN_INTERNAL_FSPY_SHM_CAPACITY=65536 vt run -v stat`

64 KiB, a thousand slots for twenty thousand accesses

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
      → Not cached: tracking ran out of room for this task's file accesses
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `VP_RUN_INTERNAL_FSPY_SHM_CAPACITY=65536 vt run -v stat`

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
      → Not cached: tracking ran out of room for this task's file accesses
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
