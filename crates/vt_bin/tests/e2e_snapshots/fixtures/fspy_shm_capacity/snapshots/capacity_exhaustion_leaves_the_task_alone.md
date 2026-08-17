# capacity_exhaustion_leaves_the_task_alone

A task that reports more file accesses than its tracking channel can hold keeps running to the end: the accesses go unrecorded, but nothing kills the process over it. The run is not cached, because the accesses that did arrive are only some of the ones the task made, and the second run is a miss for the same reason rather than replaying a wrong entry.

## `VP_RUN_INTERNAL_FSPY_SHM_CAPACITY=65536 vt run -v stat`

channel too small for the task's accesses

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
      → Not cached: the task made more file accesses than could be tracked
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run -v stat`

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
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
