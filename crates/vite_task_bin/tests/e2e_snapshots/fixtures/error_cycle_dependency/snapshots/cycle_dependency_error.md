# cycle_dependency_error

Tests error message for cyclic task dependencies

## `vt run task-a`

task-a -> task-b -> task-a cycle

**Exit code:** 1

```
Error: Cycle dependency detected: error-cycle-dependency-test#task-a -> error-cycle-dependency-test#task-b -> error-cycle-dependency-test#task-a
```
