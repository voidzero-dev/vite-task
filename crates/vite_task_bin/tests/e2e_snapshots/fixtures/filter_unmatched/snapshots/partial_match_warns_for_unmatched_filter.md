# partial_match_warns_for_unmatched_filter

Tests for unmatched --filter warnings on stderr

## `vt run --filter @test/app --filter nonexistent build`

```
No packages matched the filter: nonexistent
~/packages/app$ vtt print built-app
built-app
```
