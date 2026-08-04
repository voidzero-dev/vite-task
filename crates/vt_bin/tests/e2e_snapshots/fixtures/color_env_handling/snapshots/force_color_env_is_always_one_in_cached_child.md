# force_color_env_is_always_one_in_cached_child

The parent shell sets `FORCE_COLOR=0`, but a cached task's child always sees `FORCE_COLOR=1`. Color-related env vars are not passed through by default for cached tasks — the runner injects `FORCE_COLOR=1` so cached output is always colored, and the reporter strips colors at display time when the terminal does not support them. (For uncached tasks the parent's environment flows through untouched.)

## `FORCE_COLOR=0 vt run print-force-color`

parent FORCE_COLOR=0, cached child should still see FORCE_COLOR=1

```
$ vtt print-env FORCE_COLOR
1
```
