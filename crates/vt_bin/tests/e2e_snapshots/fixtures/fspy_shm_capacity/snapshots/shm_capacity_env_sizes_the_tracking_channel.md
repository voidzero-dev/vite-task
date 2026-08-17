# shm_capacity_env_sizes_the_tracking_channel

`VP_RUN_INTERNAL_FSPY_SHM_CAPACITY` sizes the shared memory a tracked task reports its file accesses through. The task makes twenty thousand of them, and 64 MiB holds every one, so the run caches like any other.

Setting the capacity below what the task needs is what the knob exists for. That case has to wait: a channel with no room for a record currently aborts the task process, and the panic it prints carries a thread id, a toolchain path, a backtrace and a platform's own abort code, none of which snapshot the same way twice.

Not on musl, which has no preload: those builds collect through the seccomp supervisor, on the runner's own side of the boundary, so they have no shared-memory channel to fill.

## `VP_RUN_INTERNAL_FSPY_SHM_CAPACITY=67108864 vt run -v stat`

64 MiB, room for every access

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
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run -v stat`

replayed from the entry the first run stored

```
$ vtt stat-many 20000 ◉ cache hit, replaying
stat 20000


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 1 cache hits • 0 cache misses
Performance:  100% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] fspy-shm-capacity#stat: $ vtt stat-many 20000 ✓
      → Cache hit - output replayed -
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
