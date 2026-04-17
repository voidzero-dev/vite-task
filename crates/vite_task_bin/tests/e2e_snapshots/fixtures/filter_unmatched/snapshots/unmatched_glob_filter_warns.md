# unmatched_glob_filter_warns

Tests for unmatched --filter warnings on stderr

## `vt run --filter @test/app --filter @nope/* build`

```
No packages matched the filter: @nope/*
~/packages/app$ vtt print built-app
built-app
```
