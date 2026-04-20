# last_details_with_no_previous_run_shows_error

`vt run --last-details` with no prior run in the workspace should produce a
clear error rather than empty output.

## `vt run --last-details`

**Exit code:** 1

```
No previous run summary found. Run a task first to generate a summary.
```
