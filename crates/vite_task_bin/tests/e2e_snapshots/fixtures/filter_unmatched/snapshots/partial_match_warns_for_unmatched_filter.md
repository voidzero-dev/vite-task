# partial_match_warns_for_unmatched_filter

When some `--filter` flags match and one does not, the run should proceed and
warn only for the unmatched filter.

## `vt run --filter @test/app --filter nonexistent build`

```
No packages matched the filter: nonexistent
~/packages/app$ vtt print built-app
built-app
```
