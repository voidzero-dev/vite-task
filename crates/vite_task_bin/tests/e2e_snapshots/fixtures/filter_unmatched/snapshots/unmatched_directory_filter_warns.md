# unmatched_directory_filter_warns

A directory-style `--filter` (`./packages/...`) that points nowhere should
warn just like an unmatched name filter.

## `vt run --filter @test/app --filter ./packages/nope build`

```
No packages matched the filter: ./packages/nope
~/packages/app$ vtt print built-app
built-app
```
