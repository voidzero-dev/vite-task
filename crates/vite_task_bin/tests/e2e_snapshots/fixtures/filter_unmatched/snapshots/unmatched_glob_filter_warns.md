# unmatched_glob_filter_warns

A glob `--filter` that matches no packages should warn like any other
unmatched inclusion filter.

## `vt run --filter @test/app --filter @nope/* build`

```
No packages matched the filter: @nope/*
~/packages/app$ vtt print built-app
built-app
```
