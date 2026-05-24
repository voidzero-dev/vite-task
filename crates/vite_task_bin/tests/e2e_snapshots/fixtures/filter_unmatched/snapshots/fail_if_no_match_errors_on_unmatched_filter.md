# fail_if_no_match_errors_on_unmatched_filter

With `--fail-if-no-match`, an unmatched `--filter` aborts the run with a non-zero exit code instead of warning. Mirrors pnpm's `--fail-if-no-match`.

## `vt run --filter nonexistent --fail-if-no-match build`

**Exit code:** 1

```
error: No packages matched the filter: nonexistent
```
