# unmatched_exclusion_filter_does_not_warn

Tests for unmatched --filter warnings on stderr

## `vt run --filter @test/app --filter !nonexistent build`

```
~/packages/app$ vtt print built-app
built-app
```
