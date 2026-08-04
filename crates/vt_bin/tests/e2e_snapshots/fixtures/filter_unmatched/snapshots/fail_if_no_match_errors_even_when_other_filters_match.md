# fail_if_no_match_errors_even_when_other_filters_match

Strict mode errors on **any** unmatched filter, even when other filters did match packages — this catches typos in CI scripts that combine an exact name with a glob.

## `vt run --filter @test/app --filter nonexistent --fail-if-no-match build`

**Exit code:** 1

```
error: No packages matched the filter: nonexistent
```
