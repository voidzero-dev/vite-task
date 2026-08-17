# shm_capacity_env_sizes_the_tracking_channel

`VP_RUN_INTERNAL_FSPY_SHM_CAPACITY` sizes the shared memory a tracked task reports its file accesses through. The task stats one 2 MiB path, and a 1 MiB channel has nowhere to put a record that long.

The task runs to the end anyway: recording must never stop the program doing the work, so the access goes unrecorded and the process carries on to a clean exit. What the run cannot claim is that it saw every file the task touched, so it is not cached, and the second run says the same rather than replaying an entry built from part of a trace.

## `VP_RUN_INTERNAL_FSPY_SHM_CAPACITY=1048576 vt run -v stat`

1 MiB, no room for a 2 MiB record

```
$ vtt stat_long_filename 2097152


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] fspy-shm-capacity#stat: $ vtt stat_long_filename 2097152 ✓
      → Not cached: tracking ran out of room for this task's file accesses
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `VP_RUN_INTERNAL_FSPY_SHM_CAPACITY=1048576 vt run -v stat`

nothing was cached to replay

```
$ vtt stat_long_filename 2097152


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] fspy-shm-capacity#stat: $ vtt stat_long_filename 2097152 ✓
      → Not cached: tracking ran out of room for this task's file accesses
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
