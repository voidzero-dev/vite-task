# multiple_unmatched_filters_warn_individually

Each unmatched `--filter` should produce its own warning — warnings must not
be collapsed into a single message.

## `vt run --filter @test/app --filter nope1 --filter nope2 build`

```
No packages matched the filter: nope1
No packages matched the filter: nope2
~/packages/app$ vtt print built-app
built-app
```
