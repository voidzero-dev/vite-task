# nested_filter_no_match_succeeds_by_default

The default-success rule must apply to nested `vt run --filter ...` invocations too: a task whose command is `vt run --filter nonexistent build` should exit 0 with the warning, not fail the outer task. Guards the script wrapping that pnpm users typically write.

## `vt run filter-nonexistent`

```
No packages matched the filter: nonexistent
---
vt run: 0/0 cache hit (0%). (Run `vt run --last-details` for full details)
```
