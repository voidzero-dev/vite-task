# unmatched_directory_filter_warns

Tests for unmatched --filter warnings on stderr

## `vt run --filter @test/app --filter ./packages/nope build`

```
No packages matched the filter: ./packages/nope
~/packages/app$ vtt print built-app
built-app
```
