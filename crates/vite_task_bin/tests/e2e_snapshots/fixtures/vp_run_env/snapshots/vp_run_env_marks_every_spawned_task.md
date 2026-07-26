# vp_run_env_marks_every_spawned_task

Every process a task spawns gets `VP_RUN=1`, so a tool can tell it was started by `vp run` instead of being typed directly.

The parent shell sets `VP_RUN=0` in both steps to show the marker is forced rather than inherited. The cached task also restricts passthrough to `env: ["NODE_ENV"]`, which filters the parent's value out entirely; the marker is set after that filtering, so it survives either way.

## `VP_RUN=0 vt run print-marker-cached`

cached task: env passthrough cannot drop the marker

```
$ vtt print-env VP_RUN
1
```

## `VP_RUN=0 vt run print-marker-uncached`

uncached task: the parent environment flows through, but the marker still wins

```
$ vtt print-env VP_RUN ⊘ cache disabled
1
```
