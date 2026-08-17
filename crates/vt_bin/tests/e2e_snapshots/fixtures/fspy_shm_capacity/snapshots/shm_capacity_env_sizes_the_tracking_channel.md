# shm_capacity_env_sizes_the_tracking_channel

`VP_RUN_INTERNAL_FSPY_SHM_CAPACITY` sizes the shared memory a tracked task reports its file accesses through. The task stats one 2 MiB path, which is the largest single record tracking can be asked to hold, and 64 MiB holds it comfortably, so the run caches like any other.

Setting the capacity below that record is what the knob exists for. That case has to wait: a channel with no room for a record currently aborts the task process, and the panic it prints carries a thread id, a toolchain path, a backtrace and a platform's own abort code, none of which snapshot the same way twice.

## `VP_RUN_INTERNAL_FSPY_SHM_CAPACITY=67108864 vt run -v stat`

64 MiB, room to spare for a 2 MiB record

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
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run -v stat`

replayed from the entry the first run stored

```
$ vtt stat_long_filename 2097152 ◉ cache hit, replaying


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 1 cache hits • 0 cache misses
Performance:  100% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] fspy-shm-capacity#stat: $ vtt stat_long_filename 2097152 ✓
      → Cache hit - output replayed -
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
