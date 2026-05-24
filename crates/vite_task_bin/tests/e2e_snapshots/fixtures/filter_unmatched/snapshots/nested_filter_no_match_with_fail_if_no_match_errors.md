# nested_filter_no_match_with_fail_if_no_match_errors

Strict mode also propagates through nesting: a task command that adds `--fail-if-no-match` aborts the outer task when the nested filter selects nothing. The error chain identifies both the nested command and the unmatched filter source.

## `vt run filter-nonexistent-strict`

**Exit code:** 1

```
error: Failed to plan tasks from `vt run --filter nonexistent --fail-if-no-match build` in task filter-unmatched-test#filter-nonexistent-strict
* No packages matched the filter: nonexistent
```
