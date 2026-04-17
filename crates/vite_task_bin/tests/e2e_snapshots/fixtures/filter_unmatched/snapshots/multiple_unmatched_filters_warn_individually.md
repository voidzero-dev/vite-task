# multiple_unmatched_filters_warn_individually

Tests for unmatched --filter warnings on stderr

## `vt run --filter @test/app --filter nope1 --filter nope2 build`

```
No packages matched the filter: nope1
No packages matched the filter: nope2
~/packages/app$ vtt print built-app
built-app
```
