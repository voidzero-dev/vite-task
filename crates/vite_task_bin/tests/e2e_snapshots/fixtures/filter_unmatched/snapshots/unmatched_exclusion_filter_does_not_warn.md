# unmatched_exclusion_filter_does_not_warn

An exclusion filter (`!...`) that matches nothing should be silent; only
unmatched inclusion filters warn.

## `vt run --filter @test/app --filter !nonexistent build`

```
~/packages/app$ vtt print built-app
built-app
```
